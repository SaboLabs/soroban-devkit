//! Dependency acquisition layer for the Soroban DevKit (sdkt), M35.1.
//!
//! Provides a reusable abstraction for fetching package dependencies into a
//! deterministic local cache. This milestone implements **Git** acquisition
//! (via the `git` CLI) and a path (local) source that needs no fetch. The
//! [`DependencyFetcher`] trait is the seam: a future registry source can be
//! added as another implementation without touching callers.
//!
//! Design goals:
//! * No network during tests — tests use on-the-fly local git repositories.
//! * Deterministic cache layout: `<cache_root>/git/<stable-hash>/` so repeated
//!   fetches are idempotent.
//! * No authentication helpers, no registry. The Git backend shells out to the
//!   system `git` binary (assumed present, like `cargo`/`rustc`).
//! * Never builds — `fetch` only materializes source; building is the caller's
//!   responsibility.

use crate::config::Dependency;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve the `git` executable.
///
/// Normally `git` is found via `PATH`. In some spawned/subprocess environments
/// (e.g. a test harness that restricts `PATH`) the bare `"git"` name fails to
/// spawn even though git is installed. To stay robust we fall back to a set of
/// well-known absolute locations when a `PATH` lookup does not produce a runnable
/// binary. The first candidate that reports a version is used.
fn git_bin() -> String {
    for candidate in [
        "git".to_string(),
        "/usr/bin/git".to_string(),
        "/usr/local/bin/git".to_string(),
        "/bin/git".to_string(),
    ] {
        if Command::new(&candidate)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return candidate;
        }
    }
    // Fall back to the bare name; callers surface a clear error if it truly fails.
    "git".to_string()
}

/// Errors raised while acquiring a dependency.
#[derive(Debug)]
pub enum FetchError {
    /// The system `git` binary is missing or not executable.
    GitUnavailable,
    /// `git` exited non-zero while cloning/checking out.
    Git {
        args: String,
        status: Option<i32>,
        stderr: String,
    },
    /// A dependency declared neither a `path` nor a `git` source.
    NoSource(String),
    /// A `git` dependency is missing its URL.
    MissingUrl(String),
    /// Cache directory creation/relocation failed.
    Io {
        context: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::GitUnavailable => {
                write!(
                    f,
                    "the `git` executable is required but was not found on PATH"
                )
            }
            FetchError::Git {
                args,
                status,
                stderr,
            } => write!(
                f,
                "git {} failed (exit {:?}): {}",
                args,
                status,
                stderr.trim()
            ),
            FetchError::NoSource(name) => {
                write!(f, "dependency '{}' declares neither `path` nor `git`", name)
            }
            FetchError::MissingUrl(name) => {
                write!(f, "git dependency '{}' is missing a `git` URL", name)
            }
            FetchError::Io { context, source } => {
                write!(f, "I/O error {}: {}", context, source)
            }
        }
    }
}

impl std::error::Error for FetchError {}

/// The outcome of a fetch operation for one dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOutcome {
    /// The dependency name (key in `[dependencies]`).
    pub name: String,
    /// Where the source now lives on disk.
    pub local_path: PathBuf,
    /// The resolved Git commit SHA, if this was a Git source (empty for path).
    pub resolved_rev: String,
    /// True if the checkout was already present and up to date.
    pub already_present: bool,
}

/// A source a dependency can be acquired from.
pub trait DependencyFetcher {
    /// Materialize `dep` (named `name`) into the cache and return its on-disk
    /// location. `force` re-fetches/updates an existing checkout.
    fn fetch(&self, name: &str, dep: &Dependency, force: bool) -> Result<FetchOutcome, FetchError>;
}

/// Stable, filesystem-safe cache key for a Git dependency.
///
/// Derived from the URL + reference so two deps with the same source/ref share
/// a cache entry. Not cryptographic — uniqueness only needs to be stable.
fn git_cache_key(dep: &Dependency) -> String {
    let url = dep.git.clone().unwrap_or_default();
    let reference = dep
        .tag
        .clone()
        .or_else(|| dep.branch.clone())
        .or_else(|| dep.rev.clone())
        .unwrap_or_else(|| "head".to_string());
    // Cheap FNV-1a style hash; deterministic across platforms.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.as_bytes().iter().chain(reference.as_bytes()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// Git-backed dependency fetcher.
///
/// Uses the system `git` CLI. On fetch it clones (or updates) into
/// `<cache_root>/git/<key>/`, then checks out the requested tag/branch/rev.
/// When `force` is false and the checkout already exists at the right commit,
/// it is reused without re-cloning.
pub struct GitFetcher {
    /// Root directory holding all cached dependencies (e.g. `.sdkt-cache`).
    pub cache_root: PathBuf,
}

impl GitFetcher {
    /// Create a fetcher rooted at `cache_root`.
    pub fn new(cache_root: impl Into<PathBuf>) -> Self {
        Self {
            cache_root: cache_root.into(),
        }
    }

    fn git_available() -> Result<(), FetchError> {
        Command::new(git_bin())
            .arg("--version")
            .output()
            .map_err(|_| FetchError::GitUnavailable)?;
        Ok(())
    }

    /// Resolve the current HEAD commit SHA of a checkout (empty if unknown).
    fn head_rev(checkout: &Path) -> String {
        let out = Command::new(git_bin())
            .current_dir(checkout)
            .args(["rev-parse", "HEAD"])
            .output();
        match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => String::new(),
        }
    }

    /// The commit SHA that a given reference resolves to (empty on failure).
    pub fn resolved_rev_for(dep: &Dependency, checkout: &Path) -> String {
        if let Some(rev) = &dep.rev {
            return rev.clone();
        }
        let refspec = dep
            .tag
            .as_ref()
            .map(|t| format!("refs/tags/{}", t))
            .or_else(|| {
                dep.branch
                    .as_ref()
                    .map(|b| format!("refs/remotes/origin/{}", b))
            })
            .unwrap_or_default();
        if refspec.is_empty() {
            return Self::head_rev(checkout);
        }
        let out = Command::new(git_bin())
            .current_dir(checkout)
            .args(["rev-parse", &refspec])
            .output();
        match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => String::new(),
        }
    }
}

impl DependencyFetcher for GitFetcher {
    fn fetch(&self, name: &str, dep: &Dependency, force: bool) -> Result<FetchOutcome, FetchError> {
        let url = dep
            .git
            .clone()
            .ok_or_else(|| FetchError::MissingUrl(name.to_string()))?;
        if url.trim().is_empty() {
            return Err(FetchError::MissingUrl(name.to_string()));
        }

        Self::git_available()?;

        let key = git_cache_key(dep);
        let checkout = self.cache_root.join("git").join(&key);
        // Ensure the parent cache dir (`<cache_root>/git`) exists so `git clone`
        // can create the destination checkout dir itself. The checkout dir
        // must NOT be pre-created: cloning into an existing (even empty)
        // directory with cwd set to it makes git silently fail to initialize
        // `.git`, which then breaks the later checkout.
        let parent = checkout
            .parent()
            .ok_or_else(|| FetchError::Io {
                context: format!("invalid cache path {}", checkout.display()),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent"),
            })?
            .to_path_buf();
        std::fs::create_dir_all(&parent).map_err(|source| FetchError::Io {
            context: format!("creating cache dir {}", parent.display()),
            source,
        })?;

        let existing = checkout.join(".git").exists();
        if existing && !force {
            // Verify the checkout already resolves to the requested ref.
            let want = Self::resolved_rev_for(dep, &checkout);
            if !want.is_empty() && want == Self::head_rev(&checkout) {
                return Ok(FetchOutcome {
                    name: name.to_string(),
                    local_path: checkout,
                    resolved_rev: want,
                    already_present: true,
                });
            }
        }

        // Clone or update.
        let (workdir, clone_args) = if existing {
            (checkout.clone(), vec!["fetch".to_string()])
        } else {
            (
                parent,
                vec![
                    "clone".to_string(),
                    "--no-checkout".to_string(),
                    url.clone(),
                    checkout.to_string_lossy().to_string(),
                ],
            )
        };
        run_git(&workdir, &clone_args)?;

        // Checkout the requested reference.
        let checkout_ref: String = if let Some(rev) = &dep.rev {
            rev.clone()
        } else if let Some(tag) = &dep.tag {
            tag.clone()
        } else if let Some(branch) = &dep.branch {
            format!("origin/{}", branch)
        } else {
            "origin/HEAD".to_string()
        };
        run_git(
            &checkout,
            &["checkout".to_string(), "--force".to_string(), checkout_ref],
        )?;

        let resolved = Self::resolved_rev_for(dep, &checkout);
        Ok(FetchOutcome {
            name: name.to_string(),
            local_path: checkout,
            resolved_rev: resolved,
            already_present: false,
        })
    }
}

/// A no-network "fetcher" for local `path` dependencies: it only validates
/// that the path exists and returns it unchanged.
pub struct PathResolver;

impl DependencyFetcher for PathResolver {
    fn fetch(
        &self,
        name: &str,
        dep: &Dependency,
        _force: bool,
    ) -> Result<FetchOutcome, FetchError> {
        let path = dep
            .path
            .clone()
            .ok_or_else(|| FetchError::NoSource(name.to_string()))?;
        if path.trim().is_empty() {
            return Err(FetchError::NoSource(name.to_string()));
        }
        Ok(FetchOutcome {
            name: name.to_string(),
            local_path: PathBuf::from(path),
            resolved_rev: String::new(),
            already_present: true,
        })
    }
}

/// Run a `git` command, returning a [`FetchError::Git`] on failure.
fn run_git(dir: &Path, args: &[String]) -> Result<(), FetchError> {
    let out = Command::new(git_bin())
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|_| FetchError::GitUnavailable)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(FetchError::Git {
            args: args.join(" "),
            status: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Dependency, DevKitConfig, PackageConfig};
    use std::collections::HashMap;
    use std::io::Write;

    fn git_cmd(dir: &Path) -> Command {
        let mut c = Command::new("git");
        c.current_dir(dir)
            // Treat the temp checkout as safe so git operations succeed even on
            // CI runners (Windows/macOS) where the temp directory ownership can
            // trip git's "dubious ownership" protection. Applied per-command via
            // env (no global git-config mutation, no side effects).
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "safe.directory")
            .env("GIT_CONFIG_VALUE_0", "*");
        c
    }

    fn make_git_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sdkt-fetch-src-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let o = git_cmd(&dir).args(args).output().expect("git available");
            if !o.status.success() {
                panic!(
                    "git {:?} failed (exit {:?}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
                    args,
                    o.status.code(),
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
            }
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@sdkt.local"]);
        run(&["config", "user.name", "sdkt test"]);
        // v1.0.0 tag on first commit.
        let f = dir.join("lib.rs");
        let mut fh = std::fs::File::create(&f).unwrap();
        fh.write_all(b"pub fn answer() -> u32 { 42 }").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "initial"]);
        run(&["tag", "v1.0.0"]);
        // A second commit + main branch head.
        let mut fh = std::fs::File::create(&f).unwrap();
        fh.write_all(b"pub fn answer() -> u32 { 43 }").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "second"]);
        dir
    }

    fn config_with_git(
        url: &str,
        tag: Option<&str>,
        branch: Option<&str>,
        rev: Option<&str>,
    ) -> DevKitConfig {
        DevKitConfig {
            package: Some(PackageConfig {
                name: Some("app".to_string()),
                version: Some("0.1.0".to_string()),
                description: None,
            }),
            dependencies: {
                let mut m = HashMap::new();
                m.insert(
                    "dep".to_string(),
                    Dependency {
                        git: Some(url.to_string()),
                        tag: tag.map(|s| s.to_string()),
                        branch: branch.map(|s| s.to_string()),
                        rev: rev.map(|s| s.to_string()),
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        }
    }

    #[test]
    fn fetch_git_by_tag() {
        let src = make_git_repo();
        let url = src.to_string_lossy().to_string();
        let cfg = config_with_git(&url, Some("v1.0.0"), None, None);
        let dep = cfg.dependencies.get("dep").unwrap();

        let cache = std::env::temp_dir().join(format!(
            "sdkt-fetch-cache-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&cache);

        let fetcher = GitFetcher::new(&cache);
        let out = fetcher.fetch("dep", dep, false).expect("fetch ok");
        assert!(out.local_path.exists());
        assert!(out.local_path.join(".git").exists());
        assert!(!out.resolved_rev.is_empty());
        // The checked-out content should be the v1.0.0 (answer() == 42) blob.
        let lib = std::fs::read_to_string(out.local_path.join("lib.rs")).unwrap();
        assert!(lib.contains("42"), "expected tagged content, got: {}", lib);

        // Idempotent: second fetch without force reuses, already_present true.
        let out2 = fetcher.fetch("dep", dep, false).expect("fetch ok");
        assert!(out2.already_present);

        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn fetch_git_by_rev() {
        let src = make_git_repo();
        let url = src.to_string_lossy().to_string();
        let cfg = config_with_git(&url, None, None, None);
        let _dep = cfg.dependencies.get("dep").unwrap();
        // Determine the second-commit SHA by rev-parsing origin/HEAD equiv.
        let rev = {
            let o = git_cmd(&src).args(["rev-parse", "HEAD"]).output().unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let cfg2 = config_with_git(&url, None, None, Some(&rev));
        let dep2 = cfg2.dependencies.get("dep").unwrap();

        let cache = std::env::temp_dir().join(format!(
            "sdkt-fetch-cache-rev-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&cache);
        let fetcher = GitFetcher::new(&cache);
        let out = fetcher.fetch("dep", dep2, false).expect("fetch ok");
        assert_eq!(out.resolved_rev, rev);
        let lib = std::fs::read_to_string(out.local_path.join("lib.rs")).unwrap();
        assert!(lib.contains("43"), "expected head content, got: {}", lib);

        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&src);
        let _ = &cfg; // silence unused
    }

    #[test]
    fn path_resolver_returns_path() {
        let tmp = std::env::temp_dir().join("sdkt-fetch-path");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let dep = Dependency {
            path: Some(tmp.to_string_lossy().to_string()),
            ..Default::default()
        };
        let out = PathResolver.fetch("local", &dep, false).unwrap();
        assert_eq!(out.local_path, tmp);
        assert!(out.resolved_rev.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
