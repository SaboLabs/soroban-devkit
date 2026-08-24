//! Local package-manager foundation for the Soroban DevKit (sdkt), M35.0.
//!
//! This module lays the groundwork for a future package registry **without**
//! any network or remote-registry functionality. It supports:
//!
//! * a package manifest (`[package]` with `name`/`version`/`description`),
//! * local path-only dependencies (`[dependencies]` with `path = "..."`),
//! * offline validation of package metadata and the local dependency graph.
//!
//! No git, HTTP, or registry resolution is performed. The dependency-graph
//! cycle/duplicate/self checks reuse the same topological-sort algorithm that
//! `project::resolve_deploy_order` uses, extracted into [`topo_sort`] so the
//! two resolvers share one implementation.

use crate::config::{DevKitConfig, PackageConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Errors raised while validating a local package manifest/dependency graph.
#[derive(Debug, PartialEq)]
pub enum PackageError {
    /// The `[package]` section is absent, but a manifest operation needs it.
    MissingPackage,
    /// `[package]` has no `name`.
    MissingName,
    /// `[package]` has no `version`.
    MissingVersion,
    /// `[package].version` is not a valid semver (`MAJOR.MINOR.PATCH[...]`).
    InvalidVersion(String),
    /// A `[dependencies]` entry has no `path` (the only supported source).
    MissingPath(String),
    /// A `[dependencies]` entry references an unknown/non-path key (e.g. `git`).
    UnsupportedSource(String),
    /// A dependency name is also a declared package (self-dependency).
    SelfDependency(String),
    /// The dependency graph contains a cycle.
    CircularDependency(String),
    /// The dependency graph declares the same dependency more than once.
    DuplicateDependency(String),
    /// A dependency's `path` does not exist on disk.
    PathNotFound(String),
    /// A dependency declares both a `path` and a `git` source.
    MixedSources(String),
    /// A `git` dependency is missing its URL.
    MissingGitUrl(String),
    /// A `git` dependency URL is not a valid URL.
    InvalidGitUrl(String),
    /// A `git` dependency URL uses an unsupported scheme (only `https`/`http`/`git`/`ssh`).
    UnsupportedUrlScheme(String),
    /// A `git` dependency specifies more than one of `branch`/`tag`/`rev`.
    MultipleGitRefs(String),
    /// A `git` dependency specifies none of `branch`/`tag`/`rev`.
    MissingGitRef(String),
    /// A `git` dependency's `branch`/`tag`/`rev` value is empty.
    EmptyGitRef(String),
    /// Generic error carrying a free-form message (used by M38 packaging I/O).
    Other(String),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageError::MissingPackage => write!(f, "missing [package] section in manifest"),
            PackageError::MissingName => write!(f, "[package] is missing a `name`"),
            PackageError::MissingVersion => write!(f, "[package] is missing a `version`"),
            PackageError::InvalidVersion(v) => write!(
                f,
                "invalid [package] version '{}' (expected MAJOR.MINOR.PATCH)",
                v
            ),
            PackageError::MissingPath(name) => {
                write!(f, "dependency '{}' is missing a `path`", name)
            }
            PackageError::UnsupportedSource(name) => write!(
                f,
                "dependency '{}' uses an unsupported source (only local `path` is allowed)",
                name
            ),
            PackageError::SelfDependency(name) => write!(f, "package '{}' depends on itself", name),
            PackageError::CircularDependency(name) => {
                write!(f, "circular local dependency detected involving '{}'", name)
            }
            PackageError::DuplicateDependency(name) => {
                write!(
                    f,
                    "package '{}' declares the same dependency more than once",
                    name
                )
            }
            PackageError::PathNotFound(path) => {
                write!(f, "dependency path does not exist: {}", path)
            }
            PackageError::MixedSources(name) => {
                write!(f, "dependency '{}' declares both `path` and `git`", name)
            }
            PackageError::MissingGitUrl(name) => {
                write!(f, "git dependency '{}' is missing a `git` URL", name)
            }
            PackageError::InvalidGitUrl(name) => {
                write!(f, "git dependency '{}' has an invalid URL", name)
            }
            PackageError::UnsupportedUrlScheme(url) => {
                write!(
                    f,
                    "git dependency URL uses an unsupported scheme: {} (only https/http/git/ssh allowed)",
                    url
                )
            }
            PackageError::MultipleGitRefs(name) => {
                write!(
                    f,
                    "git dependency '{}' specifies more than one of `branch`/`tag`/`rev`",
                    name
                )
            }
            PackageError::MissingGitRef(name) => {
                write!(
                    f,
                    "git dependency '{}' must specify exactly one of `branch`/`tag`/`rev`",
                    name
                )
            }
            PackageError::EmptyGitRef(name) => {
                write!(
                    f,
                    "git dependency '{}' has an empty `branch`/`tag`/`rev`",
                    name
                )
            }
            PackageError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for PackageError {}

/// Validate the `[package]` manifest metadata in isolation.
///
/// Checks that `name` and `version` are present and that `version` is a valid
/// `MAJOR.MINOR.PATCH` (optionally with a pre-release/build suffix, per
/// semver.org). Returns the parsed `PackageConfig` on success, or a clear
/// [`PackageError`] otherwise. Does not touch the filesystem or network.
pub fn validate_package(config: &DevKitConfig) -> Result<PackageConfig, PackageError> {
    let pkg = config
        .package
        .as_ref()
        .ok_or(PackageError::MissingPackage)?;

    let name = pkg.name.as_ref().ok_or(PackageError::MissingName)?;
    if name.trim().is_empty() {
        return Err(PackageError::MissingName);
    }

    let version = pkg.version.as_ref().ok_or(PackageError::MissingVersion)?;
    if version.trim().is_empty() {
        return Err(PackageError::MissingVersion);
    }
    validate_version_format(version)?;

    Ok(pkg.clone())
}

/// Resolve the best available dependency version that satisfies a semver
/// constraint, given a list of `(tag, commit)` pairs (already fetched from the
/// remote via `git ls-remote --tags`). Pure: no I/O, no allocation beyond the
/// returned `Option`.
///
/// Selection rule (M37): among the remote tags whose names parse as valid
/// semver, return the one whose version *satisfies* `constraint` and is the
/// highest by semver ordering. Tags that are not valid semver (e.g. `latest`,
/// `main`) are ignored. If no tag satisfies the constraint, returns `None`.
///
/// This is the single source of truth for constraint matching; `plan_updates`
/// in `crate::sync` calls it rather than re-implementing comparator logic.
pub fn best_version_match(tags: &[(String, String)], constraint: &str) -> Option<(String, String)> {
    use semver::{Version, VersionReq};

    let req = VersionReq::parse(constraint).ok()?;
    let mut best: Option<(Version, (String, String))> = None;
    for (tag, commit) in tags {
        // Strip a leading "v" so `v1.2.0` parses as `1.2.0`.
        let bare = tag.strip_prefix('v').unwrap_or(tag);
        let ver = match Version::parse(bare) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !req.matches(&ver) {
            continue;
        }
        match &best {
            Some((bv, _)) if *bv >= ver => {}
            _ => best = Some((ver, (tag.clone(), commit.clone()))),
        }
    }
    best.map(|(_, pair)| pair)
}

/// Validate a single `[package]` version string.
///
/// Accepts `MAJOR.MINOR.PATCH` with an optional pre-release (`-x.y`) and/or
/// build metadata (`+abc`), matching the common semver shape. Rejects empty
/// strings, missing components, non-numeric components, and negative numbers.
pub fn validate_version_format(version: &str) -> Result<(), PackageError> {
    let v = version.trim();
    if v.is_empty() {
        return Err(PackageError::InvalidVersion(version.to_string()));
    }
    let core = v.split(['+', '-']).next().unwrap_or(v);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return Err(PackageError::InvalidVersion(version.to_string()));
    }
    for p in parts {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return Err(PackageError::InvalidVersion(version.to_string()));
        }
        // Reject leading zeros (semver: "01" is invalid, "0" is fine).
        if p.len() > 1 && p.starts_with('0') {
            return Err(PackageError::InvalidVersion(version.to_string()));
        }
    }
    Ok(())
}

/// Generic topological sort (Kahn's algorithm) over a string-keyed graph.
///
/// This is the single shared cycle-detection core, reused by both the contract
/// deploy-order resolver and the local package dependency resolver so the two
/// never diverge. `graph` maps each node to the set of nodes it depends on.
///
/// Returns `Ok(order)` with a deterministic ordering, or `Err(cycle_node)`
/// naming one node that could not be resolved (i.e. part of a cycle).
pub fn topo_sort(graph: &HashMap<String, Vec<String>>) -> Result<Vec<String>, String> {
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut adj_list: HashMap<String, Vec<String>> = HashMap::new();

    for node in graph.keys() {
        in_degree.entry(node.clone()).or_insert(0);
        adj_list.entry(node.clone()).or_default();
    }

    for (node, deps) in graph {
        for dep in deps {
            adj_list.entry(dep.clone()).or_default().push(node.clone());
            *in_degree.entry(node.clone()).or_insert(0) += 1;
        }
    }

    let mut queue: Vec<String> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    queue.sort();

    let mut ordered = Vec::new();
    while let Some(current) = queue.first().cloned() {
        queue.remove(0);
        ordered.push(current.clone());
        if let Some(neighbors) = adj_list.get(&current) {
            for neighbor in neighbors {
                if let Some(d) = in_degree.get_mut(neighbor) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push(neighbor.clone());
                    }
                }
            }
        }
        queue.sort();
    }

    if ordered.len() != graph.len() {
        for (node, &degree) in &in_degree {
            if degree > 0 {
                return Err(node.clone());
            }
        }
        return Err("Unknown".to_string());
    }

    Ok(ordered)
}

/// Validate the local `[dependencies]` graph.
///
/// Checks, in order, without performing any network I/O:
/// * exactly one source per dependency — a `path` OR a `git` URL (not both),
/// * for `path` deps: the path is non-empty and resolves to an existing dir,
/// * for `git` deps: a URL is present and a valid, supported-scheme URL;
///   exactly one of `branch`/`tag`/`rev` (none empty),
/// * no self-dependency (a dependency key equal to the package name),
/// * no duplicate dependency name,
/// * the dependency graph is acyclic (reuses [`topo_sort`]).
///
/// `base_dir` is the manifest's directory used to resolve relative `path`
/// values (in production this is `.`, the cwd sdkt runs from). Git deps are
/// only validated for syntax/shape here; acquisition happens in the fetch
/// layer ([`crate::fetch`]).
pub fn validate_dependencies(base_dir: &Path, config: &DevKitConfig) -> Result<(), PackageError> {
    let pkg_name = config
        .package
        .as_ref()
        .and_then(|p| p.name.clone())
        .unwrap_or_default();

    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    for (name, dep) in &config.dependencies {
        if name == &pkg_name {
            return Err(PackageError::SelfDependency(name.clone()));
        }

        if graph.contains_key(name) {
            return Err(PackageError::DuplicateDependency(name.clone()));
        }

        let is_path = dep.path.is_some();
        let is_git = dep.git.is_some();

        match (is_path, is_git) {
            (true, true) => return Err(PackageError::MixedSources(name.clone())),
            (true, false) => {
                let path = dep.path.as_ref().unwrap();
                if path.trim().is_empty() {
                    return Err(PackageError::MissingPath(name.clone()));
                }
                let full = base_dir.join(path);
                if !full.exists() {
                    return Err(PackageError::PathNotFound(full.display().to_string()));
                }
            }
            (false, true) => {
                let url = dep.git.as_ref().unwrap();
                if url.trim().is_empty() {
                    return Err(PackageError::MissingGitUrl(name.clone()));
                }
                validate_git_url(url)?;

                // Exactly one of branch/tag/rev, none empty — UNLESS a
                // `version` constraint is declared (M37), in which case the
                // constraint resolves against remote tags and no explicit ref
                // is required. An explicit ref always takes precedence and
                // makes the `version` constraint inert.
                let has_ref = dep.branch.is_some() || dep.tag.is_some() || dep.rev.is_some();
                if !has_ref {
                    if let Some(ver) = &dep.version {
                        // A `version` constraint (M37), e.g. ">=1.0, <2". It is
                        // a semver *requirement* (VersionReq), NOT a fixed
                        // MAJOR.MINOR.PATCH version — validate it parses as such.
                        if semver::VersionReq::parse(ver).is_err() {
                            return Err(PackageError::InvalidVersion(ver.clone()));
                        }
                    } else {
                        return Err(PackageError::MissingGitRef(name.clone()));
                    }
                } else {
                    // Exactly one of branch/tag/rev may be set. A `version`
                    // constraint co-declared with a ref is allowed (the ref
                    // wins and the constraint becomes inert); only ref + ref
                    // is rejected.
                    let ref_count = dep.branch.is_some() as u8
                        + dep.tag.is_some() as u8
                        + dep.rev.is_some() as u8;
                    if ref_count > 1 {
                        return Err(PackageError::MultipleGitRefs(name.clone()));
                    }
                    for v in [dep.branch.as_ref(), dep.tag.as_ref(), dep.rev.as_ref()]
                        .into_iter()
                        .flatten()
                    {
                        if v.trim().is_empty() {
                            return Err(PackageError::EmptyGitRef(name.clone()));
                        }
                    }
                }
            }
            (false, false) => return Err(PackageError::MissingPath(name.clone())),
        }

        graph.insert(name.clone(), Vec::new());
    }

    if !graph.is_empty() {
        if let Err(node) = topo_sort(&graph) {
            return Err(PackageError::CircularDependency(node));
        }
    }

    Ok(())
}

/// Validate a Git remote URL's scheme and basic shape (no network access).
///
/// Accepted forms:
/// * SCP-like `git@host:org/repo` (no scheme),
/// * URL forms with scheme `https`, `http`, `git`, or `ssh`,
/// * Local repository paths (an absolute path on the current platform, e.g.
///   `/abs/path` on Unix or `C:\abs\path` on Windows, plus `./rel`, `../rel`,
///   `~/path`), which `git clone` accepts directly and are used by
///   offline/hermetic tests.
///
/// The URL/reference must be non-empty. A bare host with no scheme and no path
/// separator (e.g. `github.com/org/repo` without a scheme) is rejected as an
/// invalid URL. This is a syntax/shape check only — no network connection is
/// made.
pub fn validate_git_url(url: &str) -> Result<(), PackageError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(PackageError::InvalidGitUrl(url.to_string()));
    }

    // SCP-like form: git@host:org/repo
    if let Some(rest) = trimmed.strip_prefix("git@") {
        let (host, _) = rest
            .split_once(':')
            .ok_or_else(|| PackageError::InvalidGitUrl(url.to_string()))?;
        if host.is_empty() {
            return Err(PackageError::InvalidGitUrl(url.to_string()));
        }
        return Ok(());
    }

    // scheme://host/... form.
    if let Some((scheme, rest)) = trimmed.split_once("://") {
        match scheme {
            "https" | "http" | "git" | "ssh" => {}
            other => return Err(PackageError::UnsupportedUrlScheme(other.to_string())),
        }
        let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
        if host.is_empty() {
            return Err(PackageError::InvalidGitUrl(url.to_string()));
        }
        return Ok(());
    }

    // Local path form: an absolute filesystem path (platform-native, e.g.
    // `/abs/path` on Unix or `C:\abs\path` on Windows), or a relative /
    // home-anchored path (`./rel`, `../rel`, `~/path`). `git clone` accepts
    // these directly. Reject bare hosts with no scheme and no path anchor
    // (e.g. `github.com/org/repo` without a scheme), which `git` would not
    // interpret as a local repository.
    if Path::new(trimmed).is_absolute() || trimmed.starts_with('.') || trimmed.starts_with('~') {
        return Ok(());
    }

    Err(PackageError::InvalidGitUrl(url.to_string()))
}

/// Validate an entire local package manifest: metadata + dependency graph.
///
/// This is what `sdkt package validate` calls. Never performs network
/// operations. Returns `Ok(())` when the manifest is valid, or the first
/// [`PackageError`] encountered (metadata checked before dependencies).
pub fn validate_manifest(base_dir: &Path, config: &DevKitConfig) -> Result<(), PackageError> {
    validate_package(config)?;
    validate_dependencies(base_dir, config)?;
    Ok(())
}

/// Parse a manifest TOML string into [`DevKitConfig`].
///
/// Thin wrapper over [`DevKitConfig::from_toml`] kept here so the package
/// command has a single obvious entry point. A malformed manifest (including a
/// duplicate `[package]`/`[dependencies]` key or an unsupported dependency
/// source key) returns an `Err` that the CLI surfaces with a clear message.
pub fn parse_manifest(content: &str) -> Result<DevKitConfig, toml::de::Error> {
    DevKitConfig::from_toml(content)
}

// ===== Milestone 38 — Packaging & publishing workflow =====
//
// `pack` bundles a resolved project (`.sdkt.toml` + `sdkt.lock` + the git
// checkouts under `.sdkt-cache/git/<key>`) into a portable offline artifact.
// `publish_plan` is a read-only readiness check built entirely on
// `verify_dependencies` + `compute_dependency_integrity` (M35.2) — no new
// verification algorithm, no network.
//
// All new git/hash/cache logic is *reused*; packing is a copy/archive operation
// over the existing checkout tree and the descriptor is produced from data the
// lock already carries.

use std::time::{SystemTime, UNIX_EPOCH};

/// One resolved dependency recorded in a [`PackageBundle`] descriptor.
///
/// Carries enough to locate and re-verify a bundled git checkout offline: the
/// cache key (`.sdkt-cache/git/<key>`), the resolved commit, and the integrity
/// hash computed by [`crate::lock::compute_dependency_integrity`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BundleEntry {
    /// Dependency name (key under `[dependencies]`).
    pub name: String,
    /// Source kind: `local` or `git`.
    pub source: String,
    /// Git remote URL (empty for local path deps).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub git_url: String,
    /// Resolved commit SHA (empty for local path deps / not-yet-fetched).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit_sha: String,
    /// Integrity hash ("sha256:<hex>") of the cached checkout's tracked tree.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integrity: String,
    /// Cache key used by [`crate::fetch::git_cache_key`] (git deps only).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cache_key: String,
    /// Declared `version` constraint (M37), if any.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
}

/// A self-contained, offline package bundle descriptor (M38).
///
/// Produced by [`pack`] and written as `package.json` inside the artifact so a
/// downstream consumer / CI can verify the bundle reproduces the original
/// `sdkt.lock` and per-dependency integrity hashes exactly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PackageBundle {
    /// Schema marker for the descriptor.
    pub schema: String,
    /// Package name (from `[package]`).
    pub name: String,
    /// Package version (from `[package]`).
    pub version: String,
    /// Artifact format: `tar.zst` or `dir`.
    pub format: String,
    /// Path to the produced artifact (file or directory).
    pub out_path: String,
    /// sha256 of the bundled `sdkt.lock` bytes (verifies lock equivalence).
    pub lock_sha256: String,
    /// Per-dependency resolved entries (for offline reconstruct + verify).
    pub entries: Vec<BundleEntry>,
    /// Bundle creation timestamp (Unix seconds; no external time crate).
    pub created_at: u64,
}

/// Result of a `publish --dry-run` readiness check (M38).
///
/// `ready` is true only when every gate passes. `checks` lists each gate with
/// its pass state and a human-readable detail, so the CLI can render a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishReadiness {
    /// True when the package is ready to publish (all checks passed).
    pub ready: bool,
    /// `(check_name, passed, detail)` for every readiness gate.
    pub checks: Vec<(String, bool, String)>,
}

impl PackageBundle {
    /// Write this descriptor as `package.json` into `dir`.
    pub fn write_descriptor(&self, dir: &Path) -> Result<PathBuf, PackageError> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| PackageError::Other(format!("serialize bundle descriptor: {}", e)))?;
        let path = dir.join("package.json");
        std::fs::write(&path, json)
            .map_err(|e| PackageError::Other(format!("write {}: {}", path.display(), e)))?;
        Ok(path)
    }
}

/// Recursively copy a directory tree (std only; no new dependency).
fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), PackageError> {
    std::fs::create_dir_all(dst)
        .map_err(|e| PackageError::Other(format!("create {}: {}", dst.display(), e)))?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| PackageError::Other(format!("read {}: {}", src.display(), e)))?
    {
        let entry = entry
            .map_err(|e| PackageError::Other(format!("read entry in {}: {}", src.display(), e)))?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).map_err(|e| {
                PackageError::Other(format!(
                    "copy {} -> {}: {}",
                    path.display(),
                    target.display(),
                    e
                ))
            })?;
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// Bundle a project into a portable offline artifact (M38).
///
/// Reads `.sdkt.toml` + `sdkt.lock` + the resolved git checkouts under
/// `.sdkt-cache/git/<key>` (reusing [`crate::fetch::git_cache_key`] layout) and
/// writes either:
/// * a directory tree at `<out>/<name>-<version>/` (`--format dir`), or
/// * a compressed tarball `<out>/<name>-<version>.tar.zst` (`--format tar.zst`).
///
/// In both cases a `package.json` descriptor is emitted recording the lock
/// sha256 and per-dependency integrity so the bundle can be verified offline.
/// This is a **copy/archive** operation only — it introduces no new caching,
/// hashing, or git-clone logic; [`crate::lock::compute_dependency_integrity`]
/// already hashes each checkout tree and is reused here.
pub fn pack(base: &Path, out: &Path, format: &str) -> Result<PackageBundle, PackageError> {
    if format != "tar.zst" && format != "dir" {
        return Err(PackageError::Other(format!(
            "unsupported --format '{}' (expected 'tar.zst' or 'dir')",
            format
        )));
    }

    let config = DevKitConfig::from_file(base.join(".sdkt.toml"))
        .map_err(|e| PackageError::Other(format!("reading .sdkt.toml: {}", e)))?;
    let pkg = config
        .package
        .as_ref()
        .ok_or_else(|| PackageError::Other("manifest has no [package] table".to_string()))?;
    let name = pkg.name.clone().unwrap_or_default();
    let version = pkg.version.clone().unwrap_or_default();
    if name.is_empty() || version.is_empty() {
        return Err(PackageError::Other(
            "package name and version are required to pack".to_string(),
        ));
    }

    let lock_path = base.join("sdkt.lock");
    if !lock_path.exists() {
        return Err(PackageError::Other(
            "sdkt.lock not found; run `sdkt package fetch` before packing".to_string(),
        ));
    }
    let lock_bytes = std::fs::read(&lock_path)
        .map_err(|e| PackageError::Other(format!("read sdkt.lock: {}", e)))?;
    let lock_sha256 = sha256_hex(&lock_bytes);

    let lock = crate::lock::read_lock(base)
        .map_err(|e| PackageError::Other(format!("parse sdkt.lock: {}", e)))?;

    let staging = out.join(format!("{}-{}", name, version));
    // Clean any prior staging dir of the same name.
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| {
            PackageError::Other(format!("clean staging {}: {}", staging.display(), e))
        })?;
    }
    std::fs::create_dir_all(&staging)
        .map_err(|e| PackageError::Other(format!("create staging {}: {}", staging.display(), e)))?;

    // Copy manifest + lock into the staging root.
    std::fs::copy(base.join(".sdkt.toml"), staging.join(".sdkt.toml"))
        .map_err(|e| PackageError::Other(format!("copy .sdkt.toml: {}", e)))?;
    std::fs::write(staging.join("sdkt.lock"), &lock_bytes)
        .map_err(|e| PackageError::Other(format!("write sdkt.lock: {}", e)))?;

    let mut entries: Vec<BundleEntry> = Vec::new();
    for entry in &lock.dependencies {
        let dep = config.dependencies.get(&entry.name);
        let integrity = match dep {
            Some(d) => crate::lock::compute_dependency_integrity(base, d),
            None => entry.integrity.clone(),
        };
        let (cache_key, src_checkout) = match dep {
            Some(d) if d.git.is_some() => {
                let key = crate::fetch::git_cache_key(d);
                let checkout = base.join(".sdkt-cache").join("git").join(&key);
                (key, Some(checkout))
            }
            _ => (String::new(), None),
        };
        // Stage the git checkout (local path deps are not in the cache; the plan
        // bundles only the resolved git checkouts — see milestone-38-plan.md §2).
        if let Some(checkout) = src_checkout {
            if checkout.join(".git").exists() {
                let dst = staging.join(".sdkt-cache").join("git").join(&cache_key);
                copy_dir_all(&checkout, &dst)?;
            }
        }
        entries.push(BundleEntry {
            name: entry.name.clone(),
            source: entry.source.clone(),
            git_url: entry.git_url.clone(),
            commit_sha: entry.commit_sha.clone(),
            integrity,
            cache_key,
            version: entry.version.clone(),
        });
    }

    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut bundle = PackageBundle {
        schema: "sdkt.package.bundle/v1".to_string(),
        name,
        version,
        format: format.to_string(),
        out_path: String::new(),
        lock_sha256,
        entries,
        created_at,
    };
    bundle.write_descriptor(&staging)?;

    // Materialize the final artifact.
    let final_out_path: PathBuf = if format == "tar.zst" {
        let tar_path = out.join(format!("{}-{}.tar.zst", bundle.name, bundle.version));
        {
            let file = std::fs::File::create(&tar_path).map_err(|e| {
                PackageError::Other(format!("create {}: {}", tar_path.display(), e))
            })?;
            let enc = zstd::Encoder::new(file, 0)
                .map_err(|e| PackageError::Other(format!("zstd encoder: {}", e)))?;
            {
                let mut builder = tar::Builder::new(enc);
                builder
                    .append_dir_all(".", &staging)
                    .map_err(|e| PackageError::Other(format!("tar build: {}", e)))?;
                let enc = builder
                    .into_inner()
                    .map_err(|e| PackageError::Other(format!("tar finalize: {}", e)))?;
                enc.finish()
                    .map_err(|e| PackageError::Other(format!("zstd finish: {}", e)))?;
            }
        }
        // Remove the staging dir; keep only the tarball.
        std::fs::remove_dir_all(&staging).map_err(|e| {
            PackageError::Other(format!("clean staging {}: {}", staging.display(), e))
        })?;
        tar_path
    } else {
        staging
    };

    bundle.out_path = final_out_path.to_string_lossy().to_string();
    Ok(bundle)
}

/// Evaluate publish readiness for a project (M38 `publish --dry-run`).
///
/// Pure read-only validation built on [`crate::lock::verify_dependencies`]
/// (M35.2) plus manifest/lock presence. Detects: missing cache, lock drift,
/// integrity mismatch, commit mismatch, reference change, and invalid package
/// state. No network, no publish.
pub fn publish_plan(base: &Path, config: &DevKitConfig) -> Result<PublishReadiness, PackageError> {
    let mut checks: Vec<(String, bool, String)> = Vec::new();

    // 1) Manifest validity (reuses the existing validator).
    match validate_manifest(base, config) {
        Ok(()) => checks.push((
            "manifest-valid".to_string(),
            true,
            "manifest validated".to_string(),
        )),
        Err(e) => checks.push((
            "manifest-valid".to_string(),
            false,
            format!("manifest invalid: {}", e),
        )),
    }

    // 2) Lock presence + consistency (reuses M35.2 verify_dependencies).
    let report = crate::lock::verify_dependencies(base, config);
    checks.push((
        "lock-present".to_string(),
        report.present,
        if report.present {
            "sdkt.lock present".to_string()
        } else {
            "sdkt.lock missing".to_string()
        },
    ));
    checks.push((
        "lock-consistent".to_string(),
        report.consistent,
        if report.consistent {
            "lock consistent with manifest + cache".to_string()
        } else {
            format!("{} drift(s) detected", report.mismatches.len())
        },
    ));

    // 3) Per-dependency drift detail (cache missing / integrity / commit / ref).
    for m in &report.mismatches {
        let detail = match m.kind {
            crate::lock::DepMismatchKind::CacheMissing => {
                format!("{}: cache checkout missing", m.name)
            }
            crate::lock::DepMismatchKind::IntegrityMismatch => {
                format!("{}: integrity mismatch", m.name)
            }
            crate::lock::DepMismatchKind::CommitMismatch => format!("{}: commit mismatch", m.name),
            crate::lock::DepMismatchKind::ReferenceChanged => {
                format!("{}: reference changed", m.name)
            }
            crate::lock::DepMismatchKind::SourceChanged => format!("{}: source changed", m.name),
            crate::lock::DepMismatchKind::MissingInLock => format!("{}: missing in lock", m.name),
            crate::lock::DepMismatchKind::NotInManifest => format!("{}: not in manifest", m.name),
            crate::lock::DepMismatchKind::PathMissing => format!("{}: local path missing", m.name),
        };
        checks.push((format!("dep:{}", m.name), false, detail));
    }

    let ready = checks.iter().all(|(_, ok, _)| *ok);
    Ok(PublishReadiness { ready, checks })
}

/// Verify a reconstructed (unpacked) bundle reproduces the original
/// `sdkt.lock` sha256 and per-git-dependency integrity exactly (M38 round-trip).
///
/// Reuses [`crate::fetch::git_bin`] to read each checkout's tree hash the same
/// way [`crate::lock::compute_dependency_integrity`] does, so no hash logic is
/// duplicated.
pub fn verify_bundle_equivalence(
    unpacked_base: &Path,
    bundle: &PackageBundle,
) -> Result<bool, PackageError> {
    let lock_path = unpacked_base.join("sdkt.lock");
    let lock_bytes = std::fs::read(&lock_path)
        .map_err(|e| PackageError::Other(format!("read sdkt.lock: {}", e)))?;
    if sha256_hex(&lock_bytes) != bundle.lock_sha256 {
        return Ok(false);
    }
    for entry in &bundle.entries {
        if entry.source != "git" || entry.cache_key.is_empty() {
            continue;
        }
        let checkout = unpacked_base
            .join(".sdkt-cache")
            .join("git")
            .join(&entry.cache_key);
        if !checkout.join(".git").exists() {
            return Ok(false);
        }
        let out = std::process::Command::new(crate::fetch::git_bin())
            .current_dir(&checkout)
            .args(["rev-parse", "HEAD^{tree}"])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let tree = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if format!("sha256:{}", tree) != entry.integrity {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// Reconstruct a project from a bundle artifact (M38 round-trip).
///
/// * `tar.zst` — decompresses (zstd) and extracts the tar into `dest`.
/// * `dir` — copies the directory tree into `dest`.
///
/// The reconstructed tree can then be verified offline with
/// [`verify_bundle_equivalence`]. No new archive/hash logic: it reuses the
/// same `tar` / `zstd` crates and the `copy_dir_all` helper used by [`pack`].
pub fn unpack(artifact: &Path, dest: &Path) -> Result<PathBuf, PackageError> {
    std::fs::create_dir_all(dest)
        .map_err(|e| PackageError::Other(format!("create {}: {}", dest.display(), e)))?;
    let name = artifact
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_tar = name.ends_with(".tar.zst") || name.ends_with(".tgz");
    if is_tar {
        let file = std::fs::File::open(artifact)
            .map_err(|e| PackageError::Other(format!("open {}: {}", artifact.display(), e)))?;
        let dec = zstd::Decoder::new(file)
            .map_err(|e| PackageError::Other(format!("zstd decoder: {}", e)))?;
        let mut ar = tar::Archive::new(dec);
        ar.unpack(dest)
            .map_err(|e| PackageError::Other(format!("tar extract: {}", e)))?;
    } else {
        // Directory-format bundle: copy its contents into `dest`.
        copy_dir_contents(artifact, dest)?;
    }
    Ok(dest.to_path_buf())
}

/// Copy the *contents* of `src` into `dst` (does not create a `src`-named
/// subdirectory), reusing [`copy_dir_all`] per entry.
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), PackageError> {
    std::fs::create_dir_all(dst)
        .map_err(|e| PackageError::Other(format!("create {}: {}", dst.display(), e)))?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| PackageError::Other(format!("read {}: {}", src.display(), e)))?
    {
        let entry = entry
            .map_err(|e| PackageError::Other(format!("read entry in {}: {}", src.display(), e)))?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).map_err(|e| {
                PackageError::Other(format!(
                    "copy {} -> {}: {}",
                    path.display(),
                    target.display(),
                    e
                ))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Dependency, LocalDependency, PackageConfig};
    use crate::fetch::DependencyFetcher;
    use std::collections::HashMap;

    fn manifest(
        name: Option<&str>,
        version: Option<&str>,
        deps: Vec<(&str, Option<&str>)>,
    ) -> DevKitConfig {
        let package = if name.is_some() || version.is_some() {
            Some(PackageConfig {
                name: name.map(|s| s.to_string()),
                version: version.map(|s| s.to_string()),
                description: None,
            })
        } else {
            None
        };
        let mut dependencies = HashMap::new();
        for (dname, dpath) in deps {
            dependencies.insert(
                dname.to_string(),
                LocalDependency {
                    path: dpath.map(|s| s.to_string()),
                    ..Default::default()
                },
            );
        }
        DevKitConfig {
            package,
            dependencies,
            ..Default::default()
        }
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sdkt-pkg-{}-{}",
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_validate_package_ok() {
        let cfg = manifest(Some("my-token"), Some("0.1.0"), vec![]);
        assert!(validate_package(&cfg).is_ok());
    }

    #[test]
    fn test_validate_missing_name() {
        let cfg = manifest(None, Some("0.1.0"), vec![]);
        assert!(matches!(
            validate_package(&cfg),
            Err(PackageError::MissingName)
        ));
    }

    #[test]
    fn test_validate_missing_version() {
        let cfg = manifest(Some("my-token"), None, vec![]);
        assert!(matches!(
            validate_package(&cfg),
            Err(PackageError::MissingVersion)
        ));
    }

    #[test]
    fn test_validate_invalid_version_format() {
        for bad in ["1", "1.2", "x.y.z", "1.2.3.4", "v1.2.3", "01.2.3"] {
            let cfg = manifest(Some("my-token"), Some(bad), vec![]);
            assert!(
                matches!(validate_package(&cfg), Err(PackageError::InvalidVersion(_))),
                "version '{}' should be invalid",
                bad
            );
        }
    }

    #[test]
    fn test_validate_valid_version_with_prerelease() {
        for ok in ["0.1.0", "1.2.3", "10.20.30", "1.0.0-alpha", "2.3.4+build.5"] {
            assert!(
                validate_version_format(ok).is_ok(),
                "version '{}' should be valid",
                ok
            );
        }
    }

    #[test]
    fn test_validate_missing_path() {
        let cfg = manifest(Some("my-token"), Some("0.1.0"), vec![("math", None)]);
        match validate_dependencies(Path::new("."), &cfg) {
            Err(PackageError::MissingPath(d)) => assert_eq!(d, "math"),
            other => panic!("Expected MissingPath, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_self_dependency() {
        let cfg = manifest(
            Some("my-token"),
            Some("0.1.0"),
            vec![("my-token", Some("libs/token"))],
        );
        match validate_dependencies(Path::new("."), &cfg) {
            Err(PackageError::SelfDependency(d)) => assert_eq!(d, "my-token"),
            other => panic!("Expected SelfDependency, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_path_not_found() {
        let cfg = manifest(
            Some("my-token"),
            Some("0.1.0"),
            vec![("math", Some("../does-not-exist-xyz"))],
        );
        assert!(matches!(
            validate_dependencies(Path::new("."), &cfg),
            Err(PackageError::PathNotFound(_))
        ));
    }

    #[test]
    fn test_validate_valid_local_deps() {
        let tmp = temp_dir("valid");
        std::fs::create_dir_all(tmp.join("math")).unwrap();
        std::fs::create_dir_all(tmp.join("auth")).unwrap();

        let cfg = manifest(
            Some("my-token"),
            Some("0.1.0"),
            vec![("math", Some("math")), ("auth", Some("auth"))],
        );
        let res = validate_dependencies(&tmp, &cfg);
        assert!(res.is_ok(), "expected ok, got {:?}", res);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_unsupported_source_no_longer_rejected_at_parse() {
        // Since M35.1 `git` is a known dependency field, so the manifest
        // parses. Validation then rejects it because it lacks a reference.
        let toml_data = "\
[package]\nname = \"my-token\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"https://github.com/example/math\"\n";
        let parsed = parse_manifest(toml_data);
        assert!(
            parsed.is_ok(),
            "git dependency must parse (validation handles the rest)"
        );
        let cfg = parsed.unwrap();
        let res = validate_dependencies(Path::new("."), &cfg);
        assert!(
            matches!(
                res,
                Err(PackageError::MissingGitRef(_))
                    | Err(PackageError::MultipleGitRefs(_))
                    | Err(PackageError::EmptyGitRef(_))
                    | Err(PackageError::MissingGitUrl(_))
                    | Err(PackageError::MixedSources(_))
            ),
            "git dependency without a reference must be rejected: {:?}",
            res
        );
    }

    #[test]
    fn test_validate_git_url_accepts_and_rejects() {
        // A platform-absolute local path (used by offline/hermetic tests).
        let abs_local = std::env::temp_dir()
            .join("sdkt-local-repo")
            .to_string_lossy()
            .into_owned();
        for ok in [
            "https://github.com/org/repo",
            "http://github.com/org/repo",
            "git://github.com/org/repo",
            "ssh://git@github.com/org/repo",
            "git@github.com:org/repo",
            abs_local.as_str(),
            "./local/repo",
            "../sibling/repo",
        ] {
            assert!(validate_git_url(ok).is_ok(), "url '{}' should be valid", ok);
        }
        for bad in [
            "",
            "ftp://github.com/org/repo",
            "file:///tmp/repo",
            "github.com/org/repo",
            "git@:org/repo",
            "https://",
        ] {
            assert!(
                validate_git_url(bad).is_err(),
                "url '{}' should be invalid",
                bad
            );
        }
    }

    #[test]
    fn test_validate_git_tag_ok() {
        let cfg = DevKitConfig {
            package: Some(PackageConfig {
                name: Some("my-token".to_string()),
                version: Some("0.1.0".to_string()),
                description: None,
            }),
            dependencies: {
                let mut m = HashMap::new();
                m.insert(
                    "token".to_string(),
                    Dependency {
                        git: Some("https://github.com/org/token".to_string()),
                        tag: Some("v1.2.0".to_string()),
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        };
        assert!(validate_dependencies(Path::new("."), &cfg).is_ok());
    }

    #[test]
    fn test_validate_git_rejects_multiple_refs() {
        let cfg = DevKitConfig {
            package: Some(PackageConfig {
                name: Some("my-token".to_string()),
                version: Some("0.1.0".to_string()),
                description: None,
            }),
            dependencies: {
                let mut m = HashMap::new();
                m.insert(
                    "token".to_string(),
                    Dependency {
                        git: Some("https://github.com/org/token".to_string()),
                        tag: Some("v1.2.0".to_string()),
                        branch: Some("main".to_string()),
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        };
        match validate_dependencies(Path::new("."), &cfg) {
            Err(PackageError::MultipleGitRefs(d)) => assert_eq!(d, "token"),
            other => panic!("Expected MultipleGitRefs, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_git_rejects_path_plus_git() {
        let cfg = DevKitConfig {
            package: Some(PackageConfig {
                name: Some("my-token".to_string()),
                version: Some("0.1.0".to_string()),
                description: None,
            }),
            dependencies: {
                let mut m = HashMap::new();
                m.insert(
                    "token".to_string(),
                    Dependency {
                        path: Some("local/token".to_string()),
                        git: Some("https://github.com/org/token".to_string()),
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        };
        match validate_dependencies(Path::new("."), &cfg) {
            Err(PackageError::MixedSources(d)) => assert_eq!(d, "token"),
            other => panic!("Expected MixedSources, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_git_rejects_unsupported_scheme() {
        let cfg = DevKitConfig {
            package: Some(PackageConfig {
                name: Some("my-token".to_string()),
                version: Some("0.1.0".to_string()),
                description: None,
            }),
            dependencies: {
                let mut m = HashMap::new();
                m.insert(
                    "token".to_string(),
                    Dependency {
                        git: Some("ftp://github.com/org/token".to_string()),
                        tag: Some("v1.2.0".to_string()),
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        };
        match validate_dependencies(Path::new("."), &cfg) {
            Err(PackageError::UnsupportedUrlScheme(s)) => {
                assert_eq!(s, "ftp")
            }
            other => panic!("Expected UnsupportedUrlScheme, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_git_rejects_empty_ref() {
        let cfg = DevKitConfig {
            package: Some(PackageConfig {
                name: Some("my-token".to_string()),
                version: Some("0.1.0".to_string()),
                description: None,
            }),
            dependencies: {
                let mut m = HashMap::new();
                m.insert(
                    "token".to_string(),
                    Dependency {
                        git: Some("https://github.com/org/token".to_string()),
                        tag: Some("".to_string()),
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        };
        match validate_dependencies(Path::new("."), &cfg) {
            Err(PackageError::EmptyGitRef(d)) => assert_eq!(d, "token"),
            other => panic!("Expected EmptyGitRef, got {:?}", other),
        }
    }

    #[test]
    fn test_duplicate_package_name_rejected_at_parse() {
        let toml_data = "\
[package]\nname = \"my-token\"\nversion = \"0.1.0\"\n\n[package]\nname = \"other\"\nversion = \"0.2.0\"\n";
        assert!(
            parse_manifest(toml_data).is_err(),
            "duplicate [package] table must be rejected"
        );
    }

    #[test]
    fn test_topo_sort_deterministic_and_cyclic() {
        let mut g = HashMap::new();
        g.insert("a".to_string(), vec!["b".to_string()]);
        g.insert("b".to_string(), vec!["c".to_string()]);
        g.insert("c".to_string(), vec![]);
        let order = topo_sort(&g).unwrap();
        assert_eq!(order.len(), 3);
        let pos = |s: &str| order.iter().position(|x| x == s).unwrap();
        assert!(pos("c") < pos("b"));
        assert!(pos("b") < pos("a"));

        let mut cyc = HashMap::new();
        cyc.insert("x".to_string(), vec!["y".to_string()]);
        cyc.insert("y".to_string(), vec!["x".to_string()]);
        assert!(topo_sort(&cyc).is_err());
    }

    #[test]
    fn test_validate_manifest_full() {
        let tmp = temp_dir("full");
        std::fs::create_dir_all(tmp.join("math")).unwrap();

        let cfg = manifest(
            Some("my-token"),
            Some("0.1.0"),
            vec![("math", Some("math"))],
        );
        assert!(validate_manifest(&tmp, &cfg).is_ok());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- M38 packaging / publish-readiness tests ----------------------------

    fn git_repo_with_tag(dir: &Path, tag: &str) -> String {
        std::fs::create_dir_all(dir)
            .unwrap_or_else(|e| panic!("create test repo dir {}: {}", dir.display(), e));
        let run = |args: &[&str]| {
            let o = std::process::Command::new(crate::fetch::git_bin())
                .current_dir(dir)
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "safe.directory")
                .env("GIT_CONFIG_VALUE_0", "*")
                .args(args)
                .output()
                .expect("git available");
            assert!(
                o.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&o.stderr)
            );
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@sdkt.local"]);
        run(&["config", "user.name", "sdkt test"]);
        std::fs::write(dir.join("lib.rs"), b"pub fn answer() -> u32 { 42 }\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "initial"]);
        run(&["tag", tag]);
        let o = std::process::Command::new(crate::fetch::git_bin())
            .current_dir(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// Build a project at `base` with one git dependency (tagged local repo),
    /// fetch it into `.sdkt-cache`, and write `sdkt.lock`. Returns the config.
    fn setup_packed_project(base: &Path, dep_url: &str) -> DevKitConfig {
        let pkg = DevKitConfig {
            package: Some(PackageConfig {
                name: Some("m38-app".to_string()),
                version: Some("0.3.0".to_string()),
                description: None,
            }),
            dependencies: {
                let mut m = HashMap::new();
                m.insert(
                    "math".to_string(),
                    Dependency {
                        git: Some(dep_url.to_string()),
                        tag: Some("v1.0.0".to_string()),
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        };
        std::fs::create_dir_all(base).unwrap();
        std::fs::write(
            base.join(".sdkt.toml"),
            "[package]\nname = \"m38-app\"\nversion = \"0.3.0\"\n\n[dependencies.math]\ngit = \"DEP_URL\"\ntag = \"v1.0.0\"\n"
                .replace("DEP_URL", dep_url),
        )
        .unwrap();

        // Fetch the dependency into the project cache.
        let cache = crate::fetch::GitFetcher::new(base.join(".sdkt-cache"));
        let outcome = cache
            .fetch("math", &pkg.dependencies["math"], false)
            .unwrap();

        // Write the lock (reuses the single lock writer).
        let lock = crate::lock::LockFile {
            version: crate::lock::LOCK_VERSION,
            deploy_order: vec![],
            contracts: vec![],
            dependencies: crate::lock::lock_dependencies_resolved(base, &pkg, &[outcome]),
        };
        crate::lock::write_lock(base, &lock).unwrap();
        pkg
    }

    #[test]
    fn pack_dir_roundtrip_preserves_lock_and_integrity() {
        let src = temp_dir("m38-src");
        git_repo_with_tag(&src, "v1.0.0");
        let url = src.to_string_lossy().replace('\\', "/");

        let base = temp_dir("m38-base-dir");
        setup_packed_project(&base, &url);

        let out = temp_dir("m38-out-dir");
        let bundle = pack(&base, &out, "dir").expect("pack dir");
        assert_eq!(bundle.format, "dir");
        assert_eq!(bundle.name, "m38-app");
        assert_eq!(bundle.version, "0.3.0");
        assert!(!bundle.entries.is_empty(), "should record the git dep");
        assert!(bundle.lock_sha256.starts_with("sha256:"));

        let reconstructed = out.join("m38-app-0.3.0");
        assert!(reconstructed.join(".sdkt.toml").exists());
        assert!(reconstructed.join("sdkt.lock").exists());
        assert!(reconstructed.join("package.json").exists());

        // The reconstructed tree must reproduce the lock + per-dep integrity.
        assert!(
            verify_bundle_equivalence(&reconstructed, &bundle).unwrap(),
            "dir round-trip must preserve lock + integrity"
        );

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn pack_tar_zst_roundtrip_preserves_lock_and_integrity() {
        let src = temp_dir("m38-src-tar");
        git_repo_with_tag(&src, "v1.0.0");
        let url = src.to_string_lossy().replace('\\', "/");

        let base = temp_dir("m38-base-tar");
        setup_packed_project(&base, &url);

        let out = temp_dir("m38-out-tar");
        let bundle = pack(&base, &out, "tar.zst").expect("pack tar.zst");
        assert_eq!(bundle.format, "tar.zst");
        let tarball = Path::new(&bundle.out_path);
        assert!(tarball.exists(), "tarball must exist");
        assert!(tarball.to_string_lossy().ends_with(".tar.zst"));

        // Reconstruct from the tarball and verify equivalence via the embedded
        // `package.json` descriptor (no double-pack).
        let reconstruct = temp_dir("m38-reconstruct-tar");
        let _ = crate::package::unpack(tarball, &reconstruct).expect("unpack");
        let desc = std::fs::read_to_string(reconstruct.join("package.json")).unwrap();
        let rebuilt: PackageBundle = serde_json::from_str(&desc).unwrap();
        assert_eq!(rebuilt.lock_sha256, bundle.lock_sha256);
        assert!(
            verify_bundle_equivalence(&reconstruct, &rebuilt).unwrap(),
            "tar.zst round-trip must preserve lock + integrity"
        );

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_dir_all(&reconstruct);
    }

    #[test]
    fn pack_rejects_unknown_format() {
        let src = temp_dir("m38-src-fmt");
        git_repo_with_tag(&src, "v1.0.0");
        let url = src.to_string_lossy().replace('\\', "/");
        let base = temp_dir("m38-base-fmt");
        setup_packed_project(&base, &url);
        let out = temp_dir("m38-out-fmt");
        let err = pack(&base, &out, "zip").unwrap_err();
        assert!(err.to_string().contains("unsupported --format"));
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn publish_plan_ready_when_consistent() {
        let src = temp_dir("m38-src-ready");
        git_repo_with_tag(&src, "v1.0.0");
        let url = src.to_string_lossy().replace('\\', "/");
        let base = temp_dir("m38-base-ready");
        let cfg = setup_packed_project(&base, &url);

        let readiness = publish_plan(&base, &cfg).expect("publish_plan");
        assert!(
            readiness.ready,
            "consistent project must be ready: {:?}",
            readiness.checks
        );
        // Every gate passes.
        assert!(readiness.checks.iter().all(|(_, ok, _)| *ok));

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn publish_plan_detects_missing_cache() {
        let src = temp_dir("m38-src-drift");
        git_repo_with_tag(&src, "v1.0.0");
        let url = src.to_string_lossy().replace('\\', "/");
        let base = temp_dir("m38-base-drift");
        let cfg = setup_packed_project(&base, &url);

        // Remove the cached git checkout to simulate drift.
        let key = crate::fetch::git_cache_key(&cfg.dependencies["math"]);
        let checkout = base.join(".sdkt-cache").join("git").join(&key);
        assert!(checkout.exists());
        std::fs::remove_dir_all(&checkout).unwrap();

        let readiness = publish_plan(&base, &cfg).expect("publish_plan");
        assert!(!readiness.ready, "missing cache must make publish unready");
        assert!(
            readiness
                .checks
                .iter()
                .any(|(n, ok, _)| n == "dep:math" && !*ok),
            "must report the math dep drift"
        );

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&base);
    }
}
