//! Package update & synchronization layer for the Soroban DevKit (sdkt), M36.0.
//!
//! After `sdkt package fetch` materializes dependencies into `.sdkt-cache` and
//! records them in `sdkt.lock`, `sdkt package update` reconciles the lock with
//! what is currently available upstream and refreshes the on-disk checkouts.
//!
//! Design goals (reuse, no duplication):
//! * `GitFetcher` / `git_cache_key` / `git_bin` do all git I/O — this module
//!   only decides *what* to fetch and *what* to write.
//! * `lock_dependencies_resolved` (in `lock.rs`) is the single writer of
//!   `DependencyLock` entries, shared with `sdkt package fetch`.
//! * `read_lock` / `write_lock` keep `sdkt.lock` stable; contract artifacts and
//!   deploy order are never touched.
//! * `validate_manifest` is reused for manifest sanity before any work.
//!
//! Reference semantics:
//! * `rev`  — immutable, already pinned; never updated.
//! * `tag`  — resolve the tag's current commit; if it differs from the lock,
//!   update.
//! * `branch` — fetch the latest branch head; if it differs from the lock,
//!   update.
//!
//! Offline-first: available commits are resolved via `git ls-remote` against
//! the declared `git` URL. For a local-path "remote" (used in tests and for
//! local mirrors) this needs no network; reaching a real remote does, and a
//! failure is reported as a clear network/git error rather than a panic.

use crate::config::{Dependency, DevKitConfig};
use crate::fetch::{DependencyFetcher, FetchError, FetchOutcome, GitFetcher};
use crate::lock::{lock_dependencies_resolved, read_lock, write_lock, DependencyLock, LockFile};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Outcome state for a single dependency during an update plan/apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// No newer commit is available; lock already matches.
    Unchanged,
    /// A newer commit was available and applied (or would be, in check/dry-run).
    Updated,
    /// A `rev` dependency — immutably pinned, never updated.
    Pinned,
    /// A `version` constraint (M37) is declared but no remote tag satisfies it.
    Constraint,
    /// Could not be resolved (missing cache, unknown reference, git error, ...).
    Error,
}

/// One dependency's update result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateChange {
    /// Dependency name (key under `[dependencies]`).
    pub name: String,
    /// Source kind (`local` / `git`).
    pub source: String,
    /// What happened to this dependency.
    pub status: UpdateStatus,
    /// Previously locked commit SHA (empty if none / not a git dep).
    pub old_commit: String,
    /// Newly resolved/available commit SHA (empty for local / pinned-with-no-remote).
    pub new_commit: String,
    /// Human-readable detail (old→new, reason, or error message).
    pub detail: String,
    /// The tag selected by the M37 version resolver (when a `version` constraint
    /// matched). Empty otherwise. Carried so `apply_updates` can fetch the exact
    /// resolved tag without re-querying the remote.
    pub resolved_tag: Option<String>,
}

/// Aggregate report for `sdkt package update` (check / dry-run / apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateReport {
    /// How many dependencies were inspected.
    pub checked: usize,
    /// How many were (or would be) updated.
    pub updated: usize,
    /// How many were unchanged.
    pub unchanged: usize,
    /// Per-dependency detail.
    pub changes: Vec<UpdateChange>,
}

/// Errors that abort an update before producing a partial result.
#[derive(Debug)]
pub enum SyncError {
    /// The `.sdkt.toml` manifest is invalid.
    InvalidManifest(String),
    /// No `sdkt.lock` exists (run `sdkt package fetch` first).
    MissingLock,
    /// The system `git` executable is required but unavailable.
    GitUnavailable,
    /// A `git` operation exited non-zero.
    Git {
        args: String,
        status: Option<i32>,
        stderr: String,
    },
    /// A `git` remote reference (tag/branch) does not exist.
    UnknownReference(String),
    /// A network failure while contacting a git remote.
    NetworkFailure(String),
    /// A `branch` dependency's local cache checkout is in a detached HEAD
    /// (not on the declared branch).
    DetachedBranch(String),
    /// A general I/O or lock-write failure.
    Io {
        context: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::InvalidManifest(m) => write!(f, "invalid manifest: {}", m),
            SyncError::MissingLock => {
                write!(f, "no sdkt.lock found; run `sdkt package fetch` first")
            }
            SyncError::GitUnavailable => {
                write!(
                    f,
                    "the `git` executable is required but was not found on PATH"
                )
            }
            SyncError::Git {
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
            SyncError::UnknownReference(r) => write!(f, "unknown git reference: {}", r),
            SyncError::NetworkFailure(m) => {
                write!(f, "network failure contacting git remote: {}", m)
            }
            SyncError::DetachedBranch(n) => write!(
                f,
                "dependency '{}' cache is detached (not on its declared branch)",
                n
            ),
            SyncError::Io { context, source } => write!(f, "I/O error {}: {}", context, source),
        }
    }
}

impl std::error::Error for SyncError {}

/// Convert a `FetchError` (from `GitFetcher`) into a `SyncError`.
impl From<FetchError> for SyncError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::GitUnavailable => SyncError::GitUnavailable,
            FetchError::Git {
                args,
                status,
                stderr,
            } => {
                if stderr.contains("Could not resolve host")
                    || stderr.contains("timed out")
                    || stderr.contains("Connection refused")
                    || stderr.contains("unable to access")
                {
                    SyncError::NetworkFailure(format!("git {} failed: {}", args, stderr.trim()))
                } else {
                    SyncError::Git {
                        args,
                        status,
                        stderr,
                    }
                }
            }
            other => SyncError::Git {
                args: String::new(),
                status: None,
                stderr: other.to_string(),
            },
        }
    }
}

/// The `.sdkt-cache` root for a given project base directory.
///
/// Every cache-resolution path (`git_checkout`, `compute_dependency_integrity`,
/// `verify_dependencies`) keys off `base.join(".sdkt-cache")`, so this helper is
/// the single source of truth for that location. `base` is the project/workspace
/// root (`.` when running the CLI from a project), never the process cwd — using
/// cwd would drop the cache inside the current directory even when that is a
/// library crate (e.g. while running `cargo test` from `crates/sdkt-core`).
fn cache_root(base: &Path) -> PathBuf {
    base.join(".sdkt-cache")
}

/// The cache checkout path for a git dependency (reuses the fetcher's keying).
fn git_checkout(base: &Path, url: &str, reference: &str) -> PathBuf {
    let key = crate::fetch::git_cache_key(&crate::config::Dependency {
        git: Some(url.to_string()),
        tag: if reference.starts_with("tag:") {
            Some(reference.trim_start_matches("tag:").to_string())
        } else {
            None
        },
        branch: if reference.starts_with("branch:") {
            Some(reference.trim_start_matches("branch:").to_string())
        } else {
            None
        },
        rev: if reference.starts_with("rev:") {
            Some(reference.trim_start_matches("rev:").to_string())
        } else {
            None
        },
        ..Default::default()
    });
    cache_root(base).join("git").join(&key)
}

/// Resolve the commit SHA a `git ls-remote` reports for a reference.
///
/// * `rev` deps return the rev verbatim (already pinned).
/// * `tag`  → `refs/tags/<tag>`
/// * `branch` → `refs/heads/<branch>`
///
/// Offline-safe for local-path remotes. Real remotes require network; failures
/// are mapped to [`SyncError::UnknownReference`] / [`SyncError::NetworkFailure`].
fn resolve_available_commit(
    url: &str,
    tag: &Option<String>,
    branch: &Option<String>,
    rev: &Option<String>,
) -> Result<String, SyncError> {
    // rev is immutable and already pinned — the "available" commit is itself.
    if let Some(r) = rev {
        return Ok(r.clone());
    }

    let reference = tag
        .as_ref()
        .map(|t| format!("refs/tags/{}", t))
        .or_else(|| branch.as_ref().map(|b| format!("refs/heads/{}", b)))
        .unwrap_or_default();
    if reference.is_empty() {
        return Err(SyncError::UnknownReference(
            "dependency declares a git source without tag/branch/rev".to_string(),
        ));
    }

    // Probe git availability first for a clear error.
    let probe = Command::new(crate::fetch::git_bin())
        .arg("--version")
        .output();
    if probe.is_err() || !probe.unwrap().status.success() {
        return Err(SyncError::GitUnavailable);
    }

    let out = Command::new(crate::fetch::git_bin())
        .args(["ls-remote", url, &reference])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let line = stdout.lines().next().unwrap_or("").trim().to_string();
            if line.is_empty() {
                return Err(SyncError::UnknownReference(reference));
            }
            // Format: "<sha>\t<ref>".
            let sha = line.split('\t').next().unwrap_or("").trim().to_string();
            if sha.is_empty() {
                return Err(SyncError::UnknownReference(reference));
            }
            Ok(sha)
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            if stderr.contains("Could not resolve host")
                || stderr.contains("timed out")
                || stderr.contains("Connection refused")
                || stderr.contains("unable to access")
            {
                Err(SyncError::NetworkFailure(stderr))
            } else {
                Err(SyncError::Git {
                    args: format!("ls-remote {} {}", url, reference),
                    status: o.status.code(),
                    stderr,
                })
            }
        }
        Err(_) => Err(SyncError::GitUnavailable),
    }
}

/// List all `(tag, commit)` pairs advertised by a git remote's tags.
///
/// Reuses `git_bin` (the same fetcher primitive as `resolve_available_commit`).
/// Offline for local-path remotes; reaches a real remote only when the declared
/// `git` URL is network-backed. A failure (git unavailable, unknown host) maps
/// to a clear `SyncError`. Tags whose ref points at a non-commit object (e.g.
/// annotated-tag peel targets are resolved; lightweight tags are commits) are
/// returned with their peeled commit SHA.
fn list_remote_tags(url: &str) -> Result<Vec<(String, String)>, SyncError> {
    // Probe git availability first for a clear error.
    let probe = Command::new(crate::fetch::git_bin())
        .arg("--version")
        .output();
    if probe.is_err() || !probe.unwrap().status.success() {
        return Err(SyncError::GitUnavailable);
    }

    let out = Command::new(crate::fetch::git_bin())
        .args(["ls-remote", "--tags", url])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let mut tags = Vec::new();
            for line in stdout.lines() {
                // Format: "<sha>\trefs/tags/<tag>" (annotated tags also emit
                // "<sha>^{}\trefs/tags/<tag>" peeled lines — skip those).
                let (sha, r#ref) = match line.split_once('\t') {
                    Some(x) => x,
                    None => continue,
                };
                if r#ref.ends_with("^{}") {
                    continue;
                }
                let tag = match r#ref.strip_prefix("refs/tags/") {
                    Some(t) => t.to_string(),
                    None => continue,
                };
                if sha.trim().is_empty() {
                    continue;
                }
                tags.push((tag, sha.trim().to_string()));
            }
            Ok(tags)
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            if stderr.contains("Could not resolve host")
                || stderr.contains("timed out")
                || stderr.contains("Connection refused")
                || stderr.contains("unable to access")
            {
                Err(SyncError::NetworkFailure(stderr))
            } else {
                Err(SyncError::Git {
                    args: format!("ls-remote --tags {}", url),
                    status: o.status.code(),
                    stderr,
                })
            }
        }
        Err(_) => Err(SyncError::GitUnavailable),
    }
}

/// Resolve a `version` constraint (M37) against a remote's tags.
///
/// Returns `Some((tag, commit))` for the highest satisfying tag, or `None` when
/// no tag satisfies the constraint. Pure selection logic lives in
/// `crate::package::best_version_match`; this wrapper only fetches the tag list
/// (via `list_remote_tags`) and forwards it, so the I/O and the comparator stay
/// in their respective single owners.
pub(crate) fn resolve_version_constraint(
    url: &str,
    constraint: &str,
) -> Result<Option<(String, String)>, SyncError> {
    let tags = list_remote_tags(url)?;
    Ok(crate::package::best_version_match(&tags, constraint))
}

/// Whether a git cache checkout for `dep` exists on disk under `.sdkt-cache`.
fn git_cache_exists(base: &Path, dep: &crate::config::Dependency) -> bool {
    let url = dep.git.clone().unwrap_or_default();
    let reference = dep
        .tag
        .clone()
        .map(|t| format!("tag:{}", t))
        .or_else(|| dep.branch.clone().map(|b| format!("branch:{}", b)))
        .or_else(|| dep.rev.clone().map(|r| format!("rev:{}", r)))
        .unwrap_or_default();
    let checkout = git_checkout(base, &url, &reference);
    checkout.join(".git").exists()
}

/// Compute the read-only update plan: what *would* change, without touching the
/// cache or the lock. This powers both `--check` and `--dry-run`.
///
/// Returns an error only for hard, fatal conditions (invalid manifest, missing
/// lock, git unavailable). Per-dependency problems (missing cache, unknown
/// reference, network) are recorded as `UpdateChange`s with `status = Error`
/// so the caller can report them without panicking.
pub fn plan_updates(base: &Path, config: &DevKitConfig) -> Result<UpdateReport, SyncError> {
    if let Err(e) = crate::package::validate_manifest(base, config) {
        return Err(SyncError::InvalidManifest(e.to_string()));
    }
    let lock = read_lock(base).map_err(|_| SyncError::MissingLock)?;
    let locked: std::collections::HashMap<&str, &DependencyLock> = lock
        .dependencies
        .iter()
        .map(|d| (d.name.as_str(), d))
        .collect();

    let mut changes = Vec::new();
    for (name, dep) in &config.dependencies {
        let old = locked
            .get(name.as_str())
            .map(|d| d.commit_sha.clone())
            .unwrap_or_default();

        if dep.git.is_none() {
            changes.push(UpdateChange {
                name: name.clone(),
                source: "local".to_string(),
                status: UpdateStatus::Unchanged,
                old_commit: String::new(),
                new_commit: String::new(),
                detail: "local path dependency (no remote update)".to_string(),
                resolved_tag: None,
            });
            continue;
        }

        let url = dep.git.clone().unwrap_or_default();

        // rev: immutably pinned.
        if dep.rev.is_some() {
            changes.push(UpdateChange {
                name: name.clone(),
                source: "git".to_string(),
                status: UpdateStatus::Pinned,
                old_commit: old,
                new_commit: dep.rev.clone().unwrap_or_default(),
                detail: "pinned to rev (immutable)".to_string(),
                resolved_tag: None,
            });
            continue;
        }

        // M37 — version-constraint resolution: a `version` constraint with no
        // explicit tag/branch/rev resolves against the remote's tags.
        if dep.version.is_some() && dep.tag.is_none() && dep.branch.is_none() && dep.rev.is_none() {
            let constraint = dep.version.clone().unwrap();
            match resolve_version_constraint(&url, &constraint) {
                Ok(Some((tag, commit))) => {
                    let changed = !old.is_empty() && commit != old;
                    let first_lock = old.is_empty();
                    let (status, detail) = if first_lock {
                        (
                            UpdateStatus::Updated,
                            format!(
                                "not yet locked — would record {} (satisfies '{}')",
                                tag, constraint
                            ),
                        )
                    } else if changed {
                        (
                            UpdateStatus::Updated,
                            format!("update-available ({} satisfies '{}')", tag, constraint),
                        )
                    } else {
                        (
                            UpdateStatus::Unchanged,
                            format!("up-to-date ({} satisfies '{}')", tag, constraint),
                        )
                    };
                    changes.push(UpdateChange {
                        name: name.clone(),
                        source: "git".to_string(),
                        status,
                        old_commit: old,
                        new_commit: commit,
                        detail,
                        resolved_tag: Some(tag),
                    });
                }
                Ok(None) => {
                    changes.push(UpdateChange {
                        name: name.clone(),
                        source: "git".to_string(),
                        status: UpdateStatus::Constraint,
                        old_commit: old,
                        new_commit: String::new(),
                        detail: format!(
                            "constraint '{}' unsatisfied by available tags",
                            constraint
                        ),
                        resolved_tag: None,
                    });
                }
                Err(e) => {
                    let mut c = UpdateChange {
                        name: name.clone(),
                        source: "git".to_string(),
                        status: UpdateStatus::Error,
                        old_commit: old,
                        new_commit: String::new(),
                        detail: e.to_string(),
                        resolved_tag: None,
                    };
                    if !git_cache_exists(base, dep) {
                        c.detail = format!("missing cache (run `sdkt package fetch`): {}", e);
                    }
                    changes.push(c);
                }
            }
            continue;
        }

        // Resolve what the remote currently offers.
        let available = match resolve_available_commit(&url, &dep.tag, &dep.branch, &dep.rev) {
            Ok(c) => c,
            Err(e) => {
                changes.push(UpdateChange {
                    name: name.clone(),
                    source: "git".to_string(),
                    status: UpdateStatus::Error,
                    old_commit: old,
                    new_commit: String::new(),
                    detail: e.to_string(),
                    resolved_tag: None,
                });
                // Missing cache is a fatal-ish condition for check/dry-run:
                // we cannot fetch in those modes, so report it clearly.
                if !git_cache_exists(base, dep) {
                    changes.last_mut().unwrap().detail =
                        format!("missing cache (run `sdkt package fetch`): {}", e);
                }
                continue;
            }
        };

        let changed = !old.is_empty() && available != old;
        let first_lock = old.is_empty();
        if changed || first_lock {
            let detail = if first_lock {
                "not yet locked — would record available commit".to_string()
            } else {
                format!(
                    "available commit {} != locked {}",
                    &available[..available.len().min(12)],
                    &old[..old.len().min(12)]
                )
            };
            changes.push(UpdateChange {
                name: name.clone(),
                source: "git".to_string(),
                status: UpdateStatus::Updated,
                old_commit: old,
                new_commit: available,
                detail,
                resolved_tag: None,
            });
        } else {
            changes.push(UpdateChange {
                name: name.clone(),
                source: "git".to_string(),
                status: UpdateStatus::Unchanged,
                old_commit: old,
                new_commit: available,
                detail: "already at latest".to_string(),
                resolved_tag: None,
            });
        }
    }

    Ok(summarize(changes))
}

/// Apply updates: fetch/refresh every git dependency whose available commit
/// differs from the lock (tag/branch only; `rev` is skipped as pinned), then
/// rewrite `sdkt.lock` with the new commit, cache location, and integrity —
/// preserving contract artifacts and deploy order.
///
/// Local path deps and pinned `rev` deps are left untouched. Returns the
/// report plus the rewritten lock file (also written to disk).
pub fn apply_updates(
    base: &Path,
    config: &DevKitConfig,
) -> Result<(UpdateReport, LockFile), SyncError> {
    if let Err(e) = crate::package::validate_manifest(base, config) {
        return Err(SyncError::InvalidManifest(e.to_string()));
    }
    let plan = plan_updates(base, config)?;

    // Only git tag/branch deps flagged as Updated need a real fetch.
    let mut fetched: Vec<FetchOutcome> = Vec::new();
    // Cache is rooted at `base` (the project/workspace root), consistent with
    // every other cache-resolution path — NOT the process cwd, which would drop
    // the cache inside a library crate when running unit tests from there.
    let cache = cache_root(base);
    let fetcher = GitFetcher::new(cache);

    for change in &plan.changes {
        if change.status != UpdateStatus::Updated {
            continue;
        }
        let Some(dep) = config.dependencies.get(&change.name) else {
            continue;
        };
        if dep.git.is_none() {
            continue;
        }
        // M37 — a `version`-constrained dep resolved to a specific tag during
        // planning. Override the dep's ref with the resolved tag so the existing
        // `GitFetcher` (which requires exactly one ref) fetches the right commit,
        // without re-implementing fetch logic.
        let fetch_dep = if let Some(tag) = &change.resolved_tag {
            Dependency {
                git: dep.git.clone(),
                tag: Some(tag.clone()),
                branch: None,
                rev: None,
                version: None,
                ..Default::default()
            }
        } else {
            dep.clone()
        };
        // Detached-branch guard for branch deps: if the cache checkout exists
        // but is not on the declared branch, flag it rather than silently moving.
        if fetch_dep.branch.is_some() {
            let checkout = {
                let url = fetch_dep.git.clone().unwrap_or_default();
                let reference = format!("branch:{}", fetch_dep.branch.clone().unwrap());
                git_checkout(base, &url, &reference)
            };
            if checkout.join(".git").exists() {
                let sym = Command::new(crate::fetch::git_bin())
                    .current_dir(&checkout)
                    .args(["symbolic-ref", "-q", "HEAD"])
                    .output();
                let on_branch = sym.map(|o| o.status.success()).unwrap_or(false);
                if !on_branch {
                    return Err(SyncError::DetachedBranch(change.name.clone()));
                }
            }
        }
        // Refresh the cached checkout (force: pull latest). rev is never here.
        let outcome = fetcher.fetch(&change.name, &fetch_dep, true)?;
        fetched.push(outcome);
    }

    // Rebuild dependency lock entries, overlaying freshly fetched outcomes.
    let mut lock = read_lock(base).map_err(|_| SyncError::MissingLock)?;
    let new_deps = lock_dependencies_resolved(base, config, &fetched);
    lock.dependencies = new_deps;
    write_lock(base, &lock).map_err(|e| SyncError::Io {
        context: format!("writing sdkt.lock: {}", e),
        source: std::io::Error::other(e.to_string()),
    })?;

    Ok((plan, lock))
}

/// Fold a list of changes into a summarized [`UpdateReport`].
fn summarize(changes: Vec<UpdateChange>) -> UpdateReport {
    let checked = changes.len();
    let updated = changes
        .iter()
        .filter(|c| c.status == UpdateStatus::Updated)
        .count();
    let unchanged = changes
        .iter()
        .filter(|c| {
            c.status == UpdateStatus::Unchanged
                || c.status == UpdateStatus::Pinned
                || c.status == UpdateStatus::Error
        })
        .count();
    UpdateReport {
        checked,
        updated,
        unchanged,
        changes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Dependency, PackageConfig};
    use std::collections::HashMap;
    use std::io::Write;

    // --- local git repo helpers (offline, no network) ------------------------

    fn git_cmd(dir: &Path) -> Command {
        let mut c = Command::new("git");
        c.current_dir(dir)
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "safe.directory")
            .env("GIT_CONFIG_VALUE_0", "*");
        c
    }

    fn fresh_repo_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "sdkt-sync-src-{}-{}-{}",
            std::process::id(),
            nanos,
            n
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn commit(repo: &Path, content: &[u8]) {
        {
            let mut fh = std::fs::File::create(repo.join("lib.rs")).unwrap();
            fh.write_all(content).unwrap();
        }
        git_cmd(repo).args(["add", "lib.rs"]).output().unwrap();
        git_cmd(repo)
            .args(["commit", "-q", "-m", "update"])
            .output()
            .unwrap();
    }

    fn make_repo() -> PathBuf {
        let dir = fresh_repo_dir();
        git_cmd(&dir).args(["init", "-q"]).output().unwrap();
        git_cmd(&dir)
            .args(["config", "user.email", "t@sdkt.local"])
            .output()
            .unwrap();
        git_cmd(&dir)
            .args(["config", "user.name", "sdkt test"])
            .output()
            .unwrap();
        commit(&dir, b"pub fn answer() -> u32 { 42 }");
        dir
    }

    fn head(repo: &Path) -> String {
        let o = git_cmd(repo).args(["rev-parse", "HEAD"]).output().unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn config_with(name: &str, dep: Dependency) -> DevKitConfig {
        let mut deps = HashMap::new();
        deps.insert(name.to_string(), dep);
        DevKitConfig {
            package: Some(PackageConfig {
                name: Some("app".to_string()),
                version: Some("0.1.0".to_string()),
                description: None,
            }),
            dependencies: deps,
            ..Default::default()
        }
    }

    fn write_lock_with(base: &Path, deps: Vec<DependencyLock>) {
        let lock = LockFile {
            version: crate::lock::LOCK_VERSION,
            deploy_order: vec![],
            contracts: vec![],
            dependencies: deps,
        };
        crate::lock::write_lock(base, &lock).unwrap();
    }

    fn git_dep(
        url: &str,
        tag: Option<&str>,
        branch: Option<&str>,
        rev: Option<&str>,
    ) -> Dependency {
        Dependency {
            git: Some(url.to_string()),
            tag: tag.map(|s| s.to_string()),
            branch: branch.map(|s| s.to_string()),
            rev: rev.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    /// A `git` dependency constrained only by a `version` semver constraint
    /// (M37) — no explicit `tag`/`branch`/`rev`.
    fn git_dep_version(url: &str, version: &str) -> Dependency {
        Dependency {
            git: Some(url.to_string()),
            version: Some(version.to_string()),
            ..Default::default()
        }
    }

    // --- tests ---------------------------------------------------------------

    #[test]
    fn plan_pinned_rev_never_updates() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        let pin = head(&src);
        // Move the remote so a branch/tag WOULD see a change, but rev stays put.
        commit(&src, b"pub fn answer() -> u32 { 43 }");

        let cfg = config_with("dep", git_dep(&url, None, None, Some(&pin)));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-rev-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        write_lock_with(
            &base,
            vec![DependencyLock {
                name: "dep".to_string(),
                source: "git".to_string(),
                git_url: url.clone(),
                commit_sha: pin.clone(),
                ..Default::default()
            }],
        );

        let rep = plan_updates(&base, &cfg).unwrap();
        assert_eq!(rep.changes.len(), 1);
        assert_eq!(rep.changes[0].status, UpdateStatus::Pinned);
        assert_eq!(rep.changes[0].new_commit, pin);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn plan_tag_update_detected() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        // First commit is tagged v1.0.0 (from make_repo via tag below).
        git_cmd(&src).args(["tag", "v1.0.0"]).output().unwrap();
        let v1 = head(&src);
        // Move HEAD forward (the tag still points at v1, but we simulate the tag
        // being moved by re-tagging after a new commit).
        commit(&src, b"pub fn answer() -> u32 { 43 }");
        let v2 = head(&src);
        git_cmd(&src)
            .args(["tag", "-f", "v1.0.0"])
            .output()
            .unwrap();

        let cfg = config_with("dep", git_dep(&url, Some("v1.0.0"), None, None));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-tag-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // Lock records the OLD tag commit.
        write_lock_with(
            &base,
            vec![DependencyLock {
                name: "dep".to_string(),
                source: "git".to_string(),
                git_url: url.clone(),
                commit_sha: v1,
                ..Default::default()
            }],
        );

        let rep = plan_updates(&base, &cfg).unwrap();
        assert_eq!(rep.changes.len(), 1);
        assert_eq!(rep.changes[0].status, UpdateStatus::Updated);
        assert_eq!(rep.changes[0].new_commit, v2);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn plan_branch_update_detected() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        // Ensure a `main` branch exists (default branch may vary).
        git_cmd(&src)
            .args(["branch", "-M", "main"])
            .output()
            .unwrap();
        let old = head(&src);
        commit(&src, b"pub fn answer() -> u32 { 43 }");
        let new = head(&src);

        let cfg = config_with("dep", git_dep(&url, None, Some("main"), None));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-branch-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        write_lock_with(
            &base,
            vec![DependencyLock {
                name: "dep".to_string(),
                source: "git".to_string(),
                git_url: url.clone(),
                commit_sha: old,
                ..Default::default()
            }],
        );

        let rep = plan_updates(&base, &cfg).unwrap();
        assert_eq!(rep.changes[0].status, UpdateStatus::Updated);
        assert_eq!(rep.changes[0].new_commit, new);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn plan_unchanged_when_lock_matches() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        git_cmd(&src).args(["tag", "v1.0.0"]).output().unwrap();
        let v1 = head(&src);

        let cfg = config_with("dep", git_dep(&url, Some("v1.0.0"), None, None));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-unch-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        write_lock_with(
            &base,
            vec![DependencyLock {
                name: "dep".to_string(),
                source: "git".to_string(),
                git_url: url.clone(),
                commit_sha: v1,
                ..Default::default()
            }],
        );

        let rep = plan_updates(&base, &cfg).unwrap();
        assert_eq!(rep.changes[0].status, UpdateStatus::Unchanged);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn plan_missing_lock_is_error() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        let cfg = config_with("dep", git_dep(&url, Some("v1.0.0"), None, None));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-nolock-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let err = plan_updates(&base, &cfg).unwrap_err();
        assert!(matches!(err, SyncError::MissingLock));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn version_resolver_picks_highest_satisfying() {
        // Pure selection logic (no git): reuse crate::package::best_version_match.
        let tags = vec![
            ("v1.0.0".to_string(), "c1".to_string()),
            ("v1.5.0".to_string(), "c2".to_string()),
            ("v2.0.0".to_string(), "c3".to_string()),
            ("latest".to_string(), "c9".to_string()),
            ("not-semver".to_string(), "cX".to_string()),
        ];
        // Caret/range picks highest 1.x.
        let (tag, commit) =
            crate::package::best_version_match(&tags, ">=1.0, <2").expect("should satisfy");
        assert_eq!(tag, "v1.5.0");
        assert_eq!(commit, "c2");
        // Exact match.
        let (tag, _) = crate::package::best_version_match(&tags, "=2.0.0").expect("exact");
        assert_eq!(tag, "v2.0.0");
        // No match available.
        assert!(
            crate::package::best_version_match(&tags, ">=3.0").is_none(),
            "constraint >=3.0 must be unsatisfied"
        );
    }

    #[test]
    fn plan_version_update_detected() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        // Tags v1.0.0, v1.5.0, v2.0.0 (v2.0.0 is outside the constraint).
        git_cmd(&src).args(["tag", "v1.0.0"]).output().unwrap();
        let v1 = head(&src);
        commit(&src, b"pub fn answer() -> u32 { 43 }");
        git_cmd(&src).args(["tag", "v1.5.0"]).output().unwrap();
        let v15 = head(&src);
        commit(&src, b"pub fn answer() -> u32 { 44 }");
        git_cmd(&src).args(["tag", "v2.0.0"]).output().unwrap();

        // Constraint ">=1.0, <2" should resolve to v1.5.0 (highest 1.x).
        let cfg = config_with("dep", git_dep_version(&url, ">=1.0, <2"));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-ver-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        write_lock_with(
            &base,
            vec![DependencyLock {
                name: "dep".to_string(),
                source: "git".to_string(),
                git_url: url.clone(),
                commit_sha: v1,
                ..Default::default()
            }],
        );

        let rep = plan_updates(&base, &cfg).unwrap();
        assert_eq!(rep.changes.len(), 1);
        assert_eq!(rep.changes[0].status, UpdateStatus::Updated);
        assert_eq!(rep.changes[0].new_commit, v15);
        assert_eq!(rep.changes[0].resolved_tag.as_deref(), Some("v1.5.0"));
        assert!(rep.changes[0].detail.contains("update-available"));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn plan_version_constraint_unsatisfied() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        git_cmd(&src).args(["tag", "v1.0.0"]).output().unwrap();

        // Constraint ">=3.0" cannot be satisfied by the available tags.
        let cfg = config_with("dep", git_dep_version(&url, ">=3.0"));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-verbad-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        write_lock_with(
            &base,
            vec![DependencyLock {
                name: "dep".to_string(),
                source: "git".to_string(),
                git_url: url.clone(),
                commit_sha: String::new(),
                ..Default::default()
            }],
        );

        let rep = plan_updates(&base, &cfg).unwrap();
        assert_eq!(rep.changes.len(), 1);
        assert_eq!(rep.changes[0].status, UpdateStatus::Constraint);
        assert!(rep.changes[0].detail.contains("unsatisfied"));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn plan_version_up_to_date_when_locked() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        git_cmd(&src).args(["tag", "v1.0.0"]).output().unwrap();
        git_cmd(&src).args(["tag", "v1.5.0"]).output().unwrap();
        // After tagging v1.5.0, HEAD is the commit that tag points at.
        let v15 = head(&src);

        // Lock already records v1.5.0 (the constraint's best) → unchanged.
        let cfg = config_with("dep", git_dep_version(&url, ">=1.0, <2"));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-verok-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        write_lock_with(
            &base,
            vec![DependencyLock {
                name: "dep".to_string(),
                source: "git".to_string(),
                git_url: url.clone(),
                commit_sha: v15,
                ..Default::default()
            }],
        );

        let rep = plan_updates(&base, &cfg).unwrap();
        assert_eq!(rep.changes[0].status, UpdateStatus::Unchanged);
        assert!(rep.changes[0].detail.contains("up-to-date"));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn apply_updates_refreshes_cache_and_lock() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        git_cmd(&src).args(["tag", "v1.0.0"]).output().unwrap();
        let v1 = head(&src);
        commit(&src, b"pub fn answer() -> u32 { 43 }");
        let v2 = head(&src);
        git_cmd(&src)
            .args(["tag", "-f", "v1.0.0"])
            .output()
            .unwrap();

        let cfg = config_with("dep", git_dep(&url, Some("v1.0.0"), None, None));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-apply-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // Simulate a prior fetch: write lock with the OLD commit and a cache dir
        // that contains the OLD checkout (so apply_updates fetches the new one).
        let cache_key = crate::fetch::git_cache_key(&git_dep(&url, Some("v1.0.0"), None, None));
        let checkout = base.join(".sdkt-cache").join("git").join(&cache_key);
        let _ = std::fs::remove_dir_all(&checkout);
        std::fs::create_dir_all(&checkout).unwrap();
        // Clone the OLD revision into the cache so apply_updates has something to update.
        let clone = Command::new(crate::fetch::git_bin())
            .args(["clone", "-q", &url, checkout.to_string_lossy().as_ref()])
            .output()
            .unwrap();
        assert!(clone.status.success(), "seed clone failed");
        write_lock_with(
            &base,
            vec![DependencyLock {
                name: "dep".to_string(),
                source: "git".to_string(),
                git_url: url.clone(),
                commit_sha: v1,
                cache_location: checkout.display().to_string(),
                ..Default::default()
            }],
        );

        let (rep, lock) = apply_updates(&base, &cfg).unwrap();
        assert_eq!(rep.changes[0].status, UpdateStatus::Updated);
        assert_eq!(rep.changes[0].new_commit, v2);
        // Lock must now record the new commit.
        let updated = lock.dependencies.iter().find(|d| d.name == "dep").unwrap();
        assert_eq!(updated.commit_sha, v2);
        // Cache checkout must now be at the new commit.
        let cur = head(&checkout);
        assert_eq!(cur, v2);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn plan_errors_on_unknown_reference() {
        let src = make_repo();
        let url = src.to_string_lossy().to_string();
        let cfg = config_with("dep", git_dep(&url, Some("v9.9.9"), None, None));
        let base = std::env::temp_dir().join(format!(
            "sdkt-sync-unknown-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        write_lock_with(
            &base,
            vec![DependencyLock {
                name: "dep".to_string(),
                source: "git".to_string(),
                git_url: url.clone(),
                ..Default::default()
            }],
        );
        let rep = plan_updates(&base, &cfg).unwrap();
        assert_eq!(rep.changes[0].status, UpdateStatus::Error);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&src);
    }
}
