//! Project lock-file generation for the Soroban DevKit (sdkt).
//!
//! M34.1 — `sdkt.lock` generation.
//!
//! After `sdkt build` produces the workspace WASM artifacts, a `sdkt.lock`
//! file is written next to `.sdkt.toml`. It records, for every contract in the
//! resolved deployment graph, the authoritative artifact path, its SHA-256
//! hash, and the deterministic deploy order produced by
//! [`crate::project::resolve_deploy_order`].
//!
//! The lock is **advisory**: `sdkt project deploy` still works without it, and
//! a stale/mismatched lock only produces a warning (never a hard failure), so
//! existing workflows and CI that build fresh artifacts remain unaffected.

use crate::config::DevKitConfig;
use crate::project::{resolve_deploy_order, ProjectError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// One locked contract entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockEntry {
    /// Contract alias (matches `[contracts.<alias>]` in `.sdkt.toml`).
    pub alias: String,
    /// Source directory path (relative to the workspace root / `.sdkt.toml`).
    pub path: String,
    /// Resolved WASM artifact path (relative to the workspace root).
    pub artifact: String,
    /// Lowercase hex SHA-256 of the artifact bytes.
    pub sha256: String,
    /// Position in the deterministic deploy order (0 = first to deploy).
    pub order: usize,
}

/// One locked dependency entry (M35.0 / M35.1 / M35.2).
///
/// Mirrors a resolved `[dependencies.*]` entry from `.sdkt.toml`. Local path
/// deps record nothing but their source kind and absolute-ish path; Git deps
/// record the URL, the requested reference, the resolved commit SHA (when
/// known), the on-disk cache location, and an integrity hash of the cached
/// checkout when available. This is the seam where a future registry source
/// would add its own fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DependencyLock {
    /// Dependency name (key under `[dependencies]`).
    pub name: String,
    /// Source kind: `local` or `git`.
    pub source: String,
    /// Original source specifier as declared in `.sdkt.toml`:
    /// the local path (path deps) or the git URL (git deps).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub original_source: String,
    /// Git remote URL (empty for local path deps).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub git_url: String,
    /// Requested reference: `tag`/`branch`/`rev` value (empty for path deps).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolved_reference: String,
    /// Resolved commit SHA (empty if not available / not a Git dep).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit_sha: String,
    /// On-disk cache location for the resolved source (relative to the
    /// workspace root when sensible). Empty when not materialized.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cache_location: String,
    /// Integrity hash ("sha256:<hex>") of the cached checkout's tracked tree,
    /// when computed. Empty when not available (e.g. un-fetched path dep).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integrity: String,
}

/// The on-disk `sdkt.lock` structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockFile {
    /// Schema version. Bump if the layout changes incompatibly.
    pub version: u32,
    /// Deterministic deploy order (alias list, index 0 deploys first).
    pub deploy_order: Vec<String>,
    /// Per-contract locked artifacts.
    pub contracts: Vec<LockEntry>,
    /// Locked package dependencies (M35.0 / M35.1). Absent in older locks.
    #[serde(default)]
    pub dependencies: Vec<DependencyLock>,
}

/// Errors raised while generating or verifying the lock file.
#[derive(Debug)]
pub enum LockError {
    /// Underlying project resolution failure (e.g. unknown dependency).
    Project(ProjectError),
    /// A contract's WASM artifact could not be located.
    ArtifactNotFound(String),
    /// Reading/hashing an artifact failed.
    Io {
        alias: String,
        source: std::io::Error,
    },
    /// Serializing or writing the lock file failed.
    Write {
        path: PathBuf,
        source: Box<dyn std::error::Error>,
    },
    /// Parsing an existing lock file failed.
    Parse {
        path: PathBuf,
        source: Box<dyn std::error::Error>,
    },
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Project(e) => write!(f, "project resolution failed: {}", e),
            LockError::ArtifactNotFound(alias) => {
                write!(f, "no WASM artifact found for contract '{}'", alias)
            }
            LockError::Io { alias, source } => {
                write!(f, "I/O error reading artifact for '{}': {}", alias, source)
            }
            LockError::Write { path, source } => {
                write!(
                    f,
                    "failed to write lock file {}: {}",
                    path.display(),
                    source
                )
            }
            LockError::Parse { path, source } => {
                write!(
                    f,
                    "failed to parse lock file {}: {}",
                    path.display(),
                    source
                )
            }
        }
    }
}

impl std::error::Error for LockError {}

impl From<ProjectError> for LockError {
    fn from(e: ProjectError) -> Self {
        LockError::Project(e)
    }
}

/// Current `sdkt.lock` schema version.
pub const LOCK_VERSION: u32 = 1;

/// Default lock file name, written next to `.sdkt.toml`.
pub const LOCK_FILE_NAME: &str = "sdkt.lock";

/// Compute the lowercase-hex SHA-256 of a file's bytes.
pub fn compute_sha256(path: &Path) -> Result<String, LockError> {
    use sha2::Digest;
    let bytes = fs::read(path).map_err(|source| LockError::Io {
        alias: path.display().to_string(),
        source,
    })?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    Ok(format!("{:x}", digest))
}

/// Locate the single `*.wasm` artifact for a contract, matching the same logic
/// `resolve_project` uses (first `.wasm` under the release target dir). Paths in
/// `contract_path` are resolved relative to `base_dir`.
fn find_artifact(base_dir: &Path, contract_path: &str) -> Option<PathBuf> {
    let base = base_dir.join(contract_path);
    let target_dir = base
        .join("target")
        .join("wasm32-unknown-unknown")
        .join("release");
    if !target_dir.exists() {
        return None;
    }
    let entries = fs::read_dir(&target_dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|ext| ext == "wasm") {
            return Some(p);
        }
    }
    None
}

/// Generate a [`LockFile`] from the resolved project graph.
///
/// `base_dir` is the workspace root that contract `path` values in `.sdkt.toml`
/// are resolved against (in production this is `.`, the cwd sdkt runs from).
///
/// Pure-ish: resolves the deploy order from `config`, then for each contract
/// locates its WASM artifact and hashes it. Returns [`LockError::ArtifactNotFound`]
/// if any artifact is missing (e.g. `sdkt build` was not run first).
pub fn generate_lock(base_dir: &Path, config: &DevKitConfig) -> Result<LockFile, LockError> {
    let ordered = resolve_deploy_order(config)?;

    let mut contracts = Vec::with_capacity(ordered.len());
    for (order, alias) in ordered.iter().enumerate() {
        let cfg = config
            .contracts
            .get(alias)
            .expect("alias came from config.contracts");
        let artifact = find_artifact(base_dir, &cfg.path)
            .ok_or_else(|| LockError::ArtifactNotFound(alias.clone()))?;
        let sha256 = compute_sha256(&artifact)?;
        // Persist the artifact path as given in the config (relative form is
        // what `.sdkt.toml` uses and what users expect in the lock).
        let artifact_str = artifact.to_string_lossy().to_string();
        contracts.push(LockEntry {
            alias: alias.clone(),
            path: cfg.path.clone(),
            artifact: artifact_str,
            sha256,
            order,
        });
    }

    Ok(LockFile {
        version: LOCK_VERSION,
        deploy_order: ordered,
        contracts,
        dependencies: lock_dependencies(base_dir, &config.dependencies),
    })
}

/// Build [`DependencyLock`] entries from a `.sdkt.toml` dependency map.
///
/// Pure: does not fetch. For Git deps it records the URL + requested
/// reference; `commit_sha`/`cache_location`/`integrity` are filled in by the
/// caller after a fetch (or left empty when only locking the manifest). Local
/// path deps record their source path as `original_source`. This keeps
/// `sdkt.lock` stable across machines that haven't fetched yet.
///
/// `base_dir` is the workspace root used to resolve local paths so they can be
/// recorded (and later verified) as absolute-ish paths.
pub fn lock_dependencies(
    base_dir: &Path,
    deps: &std::collections::HashMap<String, crate::config::Dependency>,
) -> Vec<DependencyLock> {
    let mut out = Vec::with_capacity(deps.len());
    for (name, dep) in deps {
        if dep.git.is_some() {
            let reference = dep
                .tag
                .clone()
                .or_else(|| dep.branch.clone())
                .or_else(|| dep.rev.clone())
                .unwrap_or_default();
            out.push(DependencyLock {
                name: name.clone(),
                source: "git".to_string(),
                original_source: dep.git.clone().unwrap_or_default(),
                git_url: dep.git.clone().unwrap_or_default(),
                resolved_reference: reference,
                commit_sha: String::new(),
                cache_location: String::new(),
                integrity: String::new(),
            });
        } else {
            let path = dep.path.clone().unwrap_or_default();
            // Record the local path resolved against the workspace root so
            // verification can check existence without assuming the cwd.
            let resolved = if path.is_empty() {
                path.clone()
            } else {
                base_dir.join(&path).to_string_lossy().to_string()
            };
            out.push(DependencyLock {
                name: name.clone(),
                source: "local".to_string(),
                original_source: resolved,
                git_url: String::new(),
                resolved_reference: String::new(),
                commit_sha: String::new(),
                cache_location: String::new(),
                integrity: String::new(),
            });
        }
    }
    out
}

/// Rebuild [`DependencyLock`] entries from a set of already-resolved
/// [`FetchOutcome`]s, recording the resolved commit, cache location, and an
/// offline integrity hash for each.
///
/// This is the single source of truth used by both `sdkt package fetch` and
/// `sdkt package update` when writing dependency entries into `sdkt.lock`, so
/// the two never drift. Local `path` deps in `config` that have no
/// corresponding outcome keep their manifest-derived `original_source` (their
/// `commit_sha`/`cache_location`/`integrity` stay empty). Git deps take the
/// commit SHA and on-disk path from the matching outcome.
pub fn lock_dependencies_resolved(
    base_dir: &Path,
    config: &DevKitConfig,
    fetched: &[crate::fetch::FetchOutcome],
) -> Vec<DependencyLock> {
    let mut out = Vec::with_capacity(config.dependencies.len());
    for (name, dep) in &config.dependencies {
        if let Some(outcome) = fetched.iter().find(|o| o.name == *name) {
            let (source, original_source, git_url, resolved_reference) = if dep.git.is_some() {
                (
                    "git".to_string(),
                    dep.git.clone().unwrap_or_default(),
                    dep.git.clone().unwrap_or_default(),
                    dep.tag
                        .clone()
                        .or_else(|| dep.branch.clone())
                        .or_else(|| dep.rev.clone())
                        .unwrap_or_default(),
                )
            } else {
                let resolved = base_dir
                    .join(dep.path.clone().unwrap_or_default())
                    .to_string_lossy()
                    .to_string();
                ("local".to_string(), resolved, String::new(), String::new())
            };
            let integrity = compute_dependency_integrity(base_dir, dep);
            out.push(DependencyLock {
                name: name.clone(),
                source,
                original_source,
                git_url,
                resolved_reference,
                commit_sha: outcome.resolved_rev.clone(),
                cache_location: outcome.local_path.display().to_string(),
                integrity,
            });
        } else {
            // No fetched outcome (e.g. a local path dep, or an unchanged git dep
            // whose lock entry is preserved separately). Fall back to the pure
            // manifest-derived record so the entry still exists in the lock.
            out.push(
                lock_dependencies(base_dir, &config.dependencies)
                    .into_iter()
                    .find(|d| d.name == *name)
                    .unwrap_or_else(|| DependencyLock {
                        name: name.clone(),
                        ..Default::default()
                    }),
            );
        }
    }
    out
}

/// Read a [`LockFile`] and return its locked dependencies (empty if none).
pub fn locked_dependencies(lock: &LockFile) -> &[DependencyLock] {
    &lock.dependencies
}
pub fn lock_to_toml(lock: &LockFile) -> Result<String, LockError> {
    toml::to_string_pretty(lock).map_err(|source| LockError::Write {
        path: PathBuf::from(LOCK_FILE_NAME),
        source: Box::new(source),
    })
}

/// Write a [`LockFile`] to `dir/sdkt.lock`.
pub fn write_lock(dir: &Path, lock: &LockFile) -> Result<PathBuf, LockError> {
    let toml = lock_to_toml(lock)?;
    let path = dir.join(LOCK_FILE_NAME);
    fs::write(&path, toml).map_err(|source| LockError::Write {
        path: path.clone(),
        source: Box::new(source),
    })?;
    Ok(path)
}

/// Parse a [`LockFile`] from a TOML string.
pub fn lock_from_toml(content: &str) -> Result<LockFile, LockError> {
    toml::from_str(content).map_err(|source| LockError::Parse {
        path: PathBuf::from(LOCK_FILE_NAME),
        source: Box::new(source),
    })
}

/// Read and parse a [`LockFile`] from `dir/sdkt.lock`.
pub fn read_lock(dir: &Path) -> Result<LockFile, LockError> {
    let path = dir.join(LOCK_FILE_NAME);
    let content = fs::read_to_string(&path).map_err(|source| LockError::Parse {
        path: path.clone(),
        source: Box::new(source),
    })?;
    lock_from_toml(&content)
}

/// Outcome of a lock verification against the live artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockVerifyReport {
    /// Lock file was present and parsed successfully.
    pub present: bool,
    /// True when every locked artifact still matches its recorded hash.
    pub consistent: bool,
    /// Aliases whose hash differs from the lock (empty when consistent).
    pub mismatched: Vec<String>,
    /// Aliases in the config that are absent from the lock.
    pub missing_in_lock: Vec<String>,
}

/// Verify an existing lock file against the current on-disk artifacts.
///
/// `base_dir` is the workspace root the lock file and artifact paths are
/// resolved against (production: `.`, the cwd sdkt runs from).
///
/// This is **advisory**: it never errors on a stale lock, it simply reports
/// which artifacts drifted. A missing lock yields `present = false` with
/// `consistent = false` and `missing_in_lock` populated from the config.
pub fn verify_lock(base_dir: &Path, config: &DevKitConfig) -> LockVerifyReport {
    let Ok(lock) = read_lock(base_dir) else {
        let missing_in_lock = config.contracts.keys().cloned().collect();
        return LockVerifyReport {
            present: false,
            consistent: false,
            mismatched: vec![],
            missing_in_lock,
        };
    };

    let mut mismatched = Vec::new();
    let locked_aliases: std::collections::HashSet<String> =
        lock.contracts.iter().map(|c| c.alias.clone()).collect();
    let mut missing_in_lock = Vec::new();

    for alias in config.contracts.keys() {
        if !locked_aliases.contains(alias) {
            missing_in_lock.push(alias.clone());
        }
    }

    for entry in &lock.contracts {
        let artifact = base_dir.join(&entry.artifact);
        let Ok(current) = compute_sha256(&artifact) else {
            // Artifact gone entirely — treat as mismatch.
            mismatched.push(entry.alias.clone());
            continue;
        };
        if current != entry.sha256 {
            mismatched.push(entry.alias.clone());
        }
    }

    let consistent = mismatched.is_empty() && missing_in_lock.is_empty();
    LockVerifyReport {
        present: true,
        consistent,
        mismatched,
        missing_in_lock,
    }
}

/// A single dependency-lock mismatch discovered during verification (M35.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepMismatchKind {
    /// Dependency in the manifest is absent from the lock.
    MissingInLock,
    /// Dependency in the lock is absent from the manifest.
    NotInManifest,
    /// Source kind (`path` vs `git`) or the source locator changed.
    SourceChanged,
    /// The requested `tag`/`branch`/`rev` reference changed.
    ReferenceChanged,
    /// A Git dependency's local cache checkout is missing.
    CacheMissing,
    /// A local path dependency no longer exists on disk.
    PathMissing,
    /// A Git cache checkout resolves to a different commit than the lock.
    CommitMismatch,
    /// Optional integrity hash differs from the current checkout.
    IntegrityMismatch,
}

/// One dependency-lock drift record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepMismatch {
    /// Dependency name (key under `[dependencies]`).
    pub name: String,
    /// Which kind of drift was detected.
    pub kind: DepMismatchKind,
    /// Human-readable detail (paths, expected vs actual, etc.).
    pub detail: String,
}

/// Outcome of verifying locked package dependencies against the live manifest
/// and on-disk state (M35.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepVerifyReport {
    /// A lock file was present and parsed successfully.
    pub present: bool,
    /// True when every dependency matches the manifest and on-disk state.
    pub consistent: bool,
    /// How many dependencies were checked.
    pub checked: usize,
    /// Every drift record (empty when consistent).
    pub mismatches: Vec<DepMismatch>,
}

/// Resolve the current HEAD commit SHA of a local git checkout (empty if the
/// checkout is missing or git is unavailable). Reuses the fetcher's git
/// discovery so it works in restricted PATH environments.
fn git_head_commit(checkout: &Path) -> String {
    let out = std::process::Command::new(crate::fetch::git_bin())
        .current_dir(checkout)
        .args(["rev-parse", "HEAD"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

/// Compute a deterministic integrity string for a dependency's on-disk source.
///
/// * Git deps: `sha256:<tree-hash>` via `git rev-parse HEAD^{tree}` of the
///   cached checkout (stable across machines for the same tree).
/// * Local path deps: `sha256:<hash>` over the sorted relative file paths and
///   their contents (so a byte change anywhere in the tree is detected).
///
/// Returns an empty string when the source cannot be read (e.g. not fetched
/// yet). This is purely offline — no network, no registry.
pub fn compute_dependency_integrity(base_dir: &Path, dep: &crate::config::Dependency) -> String {
    if let Some(_git) = &dep.git {
        let key = crate::fetch::git_cache_key(dep);
        let checkout = base_dir.join(".sdkt-cache").join("git").join(&key);
        if checkout.join(".git").exists() {
            let out = std::process::Command::new(crate::fetch::git_bin())
                .current_dir(&checkout)
                .args(["rev-parse", "HEAD^{tree}"])
                .output();
            if let Ok(o) = out {
                if o.status.success() {
                    let tree = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if !tree.is_empty() {
                        return format!("sha256:{}", tree);
                    }
                }
            }
        }
        return String::new();
    }

    // Local path dependency: hash the directory tree deterministically.
    let path = dep.path.clone().unwrap_or_default();
    if path.is_empty() {
        return String::new();
    }
    let abs = base_dir.join(&path);
    if !abs.is_dir() {
        return String::new();
    }
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    let mut files = Vec::new();
    // Recursive directory walk with only stable std APIs (no extra deps).
    fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    collect(&p, out);
                } else if p.is_file() {
                    out.push(p);
                }
            }
        }
    }
    collect(&abs, &mut files);
    files.sort();
    let mut total = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(&abs)
            .unwrap_or(f)
            .to_string_lossy()
            .to_string();
        total.extend_from_slice(rel.as_bytes());
        total.push(0);
        if let Ok(bytes) = std::fs::read(f) {
            total.extend_from_slice(&bytes);
        }
        total.push(0);
    }
    hasher.update(&total);
    format!("sha256:{:x}", hasher.finalize())
}

/// Verify locked package dependencies against the live manifest and disk.
///
/// `base_dir` is the workspace root (`.` in production). Reuses
/// [`crate::package::validate_dependencies`] to confirm the manifest itself is
/// well-formed, then compares each manifest dependency to its lock entry:
///
/// * source kind + locator match (`path` vs `git`, path/git_url),
/// * the requested reference (`tag`/`branch`/`rev`) matches,
/// * local path dependencies still exist on disk,
/// * Git cache checkouts exist and resolve to the locked commit SHA.
///
/// This is **advisory**: it never errors, it reports every drift. A missing
/// lock yields `present = false` with `consistent = false` and every manifest
/// dependency listed as `MissingInLock`. Network/registry access is never
/// required — Git resolution reads only the local `.sdkt-cache` checkout.
pub fn verify_dependencies(base_dir: &Path, config: &DevKitConfig) -> DepVerifyReport {
    // Reuse the existing manifest validator rather than re-implementing
    // dependency validation. If the manifest is malformed we still report the
    // lock state; the validator's error surfaces through the normal CLI path.
    let _ = crate::package::validate_dependencies(base_dir, config);

    let Ok(lock) = read_lock(base_dir) else {
        let mismatches = config
            .dependencies
            .keys()
            .map(|name| DepMismatch {
                name: name.clone(),
                kind: DepMismatchKind::MissingInLock,
                detail: "no sdkt.lock present".to_string(),
            })
            .collect();
        return DepVerifyReport {
            present: false,
            consistent: false,
            checked: config.dependencies.len(),
            mismatches,
        };
    };

    let locked: std::collections::HashMap<&str, &DependencyLock> = lock
        .dependencies
        .iter()
        .map(|d| (d.name.as_str(), d))
        .collect();

    let mut mismatches: Vec<DepMismatch> = Vec::new();
    let mut checked = 0;

    for (name, dep) in &config.dependencies {
        checked += 1;
        let is_git = dep.git.is_some();
        let requested_ref = dep
            .tag
            .clone()
            .or_else(|| dep.branch.clone())
            .or_else(|| dep.rev.clone())
            .unwrap_or_default();

        let Some(entry) = locked.get(name.as_str()) else {
            mismatches.push(DepMismatch {
                name: name.clone(),
                kind: DepMismatchKind::MissingInLock,
                detail: "dependency not recorded in lock".to_string(),
            });
            continue;
        };

        // Source kind + locator.
        let lock_is_git = entry.source == "git";
        if is_git != lock_is_git {
            mismatches.push(DepMismatch {
                name: name.clone(),
                kind: DepMismatchKind::SourceChanged,
                detail: format!(
                    "manifest source is {}, lock recorded {}",
                    if is_git { "git" } else { "local" },
                    entry.source
                ),
            });
            continue;
        }
        if is_git {
            if entry.git_url != dep.git.clone().unwrap_or_default() {
                mismatches.push(DepMismatch {
                    name: name.clone(),
                    kind: DepMismatchKind::SourceChanged,
                    detail: format!(
                        "git url changed: manifest '{}' vs lock '{}'",
                        dep.git.clone().unwrap_or_default(),
                        entry.git_url
                    ),
                });
            }
            if entry.resolved_reference != requested_ref {
                mismatches.push(DepMismatch {
                    name: name.clone(),
                    kind: DepMismatchKind::ReferenceChanged,
                    detail: format!(
                        "reference changed: manifest '{}' vs lock '{}'",
                        requested_ref, entry.resolved_reference
                    ),
                });
            }
            // Git cache presence + commit match (only meaningful once fetched).
            if !entry.commit_sha.is_empty() {
                let key = crate::fetch::git_cache_key(dep);
                let checkout = base_dir.join(".sdkt-cache").join("git").join(&key);
                if !checkout.join(".git").exists() {
                    mismatches.push(DepMismatch {
                        name: name.clone(),
                        kind: DepMismatchKind::CacheMissing,
                        detail: format!("git cache checkout missing: {}", checkout.display()),
                    });
                } else {
                    let head = git_head_commit(&checkout);
                    if !head.is_empty() && head != entry.commit_sha {
                        mismatches.push(DepMismatch {
                            name: name.clone(),
                            kind: DepMismatchKind::CommitMismatch,
                            detail: format!(
                                "cache commit {} != locked {}",
                                &head[..head.len().min(12)],
                                &entry.commit_sha[..entry.commit_sha.len().min(12)]
                            ),
                        });
                    }
                    // Optional integrity check.
                    if !entry.integrity.is_empty() {
                        let current = compute_dependency_integrity(base_dir, dep);
                        if !current.is_empty() && current != entry.integrity {
                            mismatches.push(DepMismatch {
                                name: name.clone(),
                                kind: DepMismatchKind::IntegrityMismatch,
                                detail: format!(
                                    "integrity changed: {} != {}",
                                    current, entry.integrity
                                ),
                            });
                        }
                    }
                }
            }
        } else {
            // Local path dependency: locator match + existence.
            let resolved = base_dir
                .join(dep.path.clone().unwrap_or_default())
                .to_string_lossy()
                .to_string();
            if entry.original_source != resolved {
                mismatches.push(DepMismatch {
                    name: name.clone(),
                    kind: DepMismatchKind::SourceChanged,
                    detail: format!(
                        "path changed: manifest '{}' vs lock '{}'",
                        resolved, entry.original_source
                    ),
                });
            } else if !std::path::Path::new(&resolved).exists() {
                mismatches.push(DepMismatch {
                    name: name.clone(),
                    kind: DepMismatchKind::PathMissing,
                    detail: format!("local path dependency missing: {}", resolved),
                });
            }
        }
    }

    // Locked deps that no longer appear in the manifest.
    for entry in &lock.dependencies {
        if !config.dependencies.contains_key(&entry.name) {
            mismatches.push(DepMismatch {
                name: entry.name.clone(),
                kind: DepMismatchKind::NotInManifest,
                detail: "lock records a dependency absent from the manifest".to_string(),
            });
        }
    }

    DepVerifyReport {
        present: true,
        consistent: mismatches.is_empty(),
        checked,
        mismatches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ContractConfig, DecodeConfig, Dependency, DevKitConfig, NetworkConfig, StorageConfig,
    };
    use std::collections::HashMap;
    use std::io::Write;

    fn write_temp_wasm(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let target = dir
            .join("target")
            .join("wasm32-unknown-unknown")
            .join("release");
        fs::create_dir_all(&target).unwrap();
        let p = target.join(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    fn config_with(contracts: HashMap<String, ContractConfig>) -> DevKitConfig {
        DevKitConfig {
            network: NetworkConfig::default(),
            decode: DecodeConfig::default(),
            storage: StorageConfig::default(),
            contracts,
            ..Default::default()
        }
    }

    #[test]
    fn compute_sha256_known_vector() {
        // SHA-256 of empty input.
        let dir = std::env::temp_dir().join("sdkt_lock_test_empty");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = write_temp_wasm(&dir, "x.wasm", b"");
        let h = compute_sha256(&p).unwrap();
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_lock_records_order_and_hashes() {
        let root = std::env::temp_dir().join("sdkt_lock_test_gen");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let token = root.join("contracts/token");
        let router = root.join("contracts/router");
        fs::create_dir_all(&token).unwrap();
        fs::create_dir_all(&router).unwrap();
        let token_wasm = write_temp_wasm(&token, "token.wasm", b"token-bytes");
        let router_wasm = write_temp_wasm(&router, "router.wasm", b"router-bytes");

        let mut map = HashMap::new();
        map.insert(
            "token".to_string(),
            ContractConfig {
                path: "contracts/token".to_string(),
                deploy_after: vec![],
                depends_on: vec![],
            },
        );
        map.insert(
            "router".to_string(),
            ContractConfig {
                path: "contracts/router".to_string(),
                deploy_after: vec!["token".to_string()],
                depends_on: vec![],
            },
        );
        let config = config_with(map);

        let lock = generate_lock(&root, &config).unwrap();
        assert_eq!(lock.version, LOCK_VERSION);
        // router depends on token => token deployed first.
        assert_eq!(
            lock.deploy_order,
            vec!["token".to_string(), "router".to_string()]
        );
        assert_eq!(lock.contracts.len(), 2);

        let token_entry = lock.contracts.iter().find(|c| c.alias == "token").unwrap();
        assert_eq!(token_entry.order, 0);
        assert_eq!(token_entry.artifact, token_wasm.to_string_lossy());
        assert_eq!(token_entry.sha256, compute_sha256(&token_wasm).unwrap());

        let router_entry = lock.contracts.iter().find(|c| c.alias == "router").unwrap();
        assert_eq!(router_entry.order, 1);
        assert_eq!(router_entry.artifact, router_wasm.to_string_lossy());

        // Round-trip TOML.
        let toml = lock_to_toml(&lock).unwrap();
        let parsed = lock_from_toml(&toml).unwrap();
        assert_eq!(parsed, lock);

        // Write + read round-trip.
        let written = write_lock(&root, &lock).unwrap();
        assert!(written.exists());
        let reread = read_lock(&root).unwrap();
        assert_eq!(reread, lock);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn generate_lock_errors_when_artifact_missing() {
        let root = std::env::temp_dir().join("sdkt_lock_test_missing");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let token = root.join("contracts/token");
        fs::create_dir_all(&token).unwrap(); // no wasm built

        let mut map = HashMap::new();
        map.insert(
            "token".to_string(),
            ContractConfig {
                path: "contracts/token".to_string(),
                deploy_after: vec![],
                depends_on: vec![],
            },
        );
        let config = config_with(map);
        let res = generate_lock(&root, &config);
        assert!(matches!(res, Err(LockError::ArtifactNotFound(_))));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_lock_detects_drift_and_missing() {
        let root = std::env::temp_dir().join("sdkt_lock_test_verify");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let token = root.join("contracts/token");
        fs::create_dir_all(&token).unwrap();
        let token_wasm = write_temp_wasm(&token, "token.wasm", b"original");

        let mut map = HashMap::new();
        map.insert(
            "token".to_string(),
            ContractConfig {
                path: "contracts/token".to_string(),
                deploy_after: vec![],
                depends_on: vec![],
            },
        );
        let config = config_with(map);

        let lock = generate_lock(&root, &config).unwrap();
        write_lock(&root, &lock).unwrap();

        // Initial verify: consistent.
        let report = verify_lock(&root, &config);
        assert!(report.present);
        assert!(report.consistent);
        assert!(report.mismatched.is_empty());

        // Mutate the artifact on disk => mismatch.
        write_temp_wasm(&token, "token.wasm", b"tampered");
        let report = verify_lock(&root, &config);
        assert!(report.present);
        assert!(!report.consistent);
        assert_eq!(report.mismatched, vec!["token".to_string()]);

        // Remove the artifact entirely => mismatch.
        let _ = fs::remove_file(&token_wasm);
        let report = verify_lock(&root, &config);
        assert!(report.mismatched.contains(&"token".to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_lock_absent_reports_missing_in_lock() {
        let root = std::env::temp_dir().join("sdkt_lock_test_no_lock");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let mut map = HashMap::new();
        map.insert(
            "token".to_string(),
            ContractConfig {
                path: "contracts/token".to_string(),
                deploy_after: vec![],
                depends_on: vec![],
            },
        );
        let config = config_with(map);

        let report = verify_lock(&root, &config);
        assert!(!report.present);
        assert!(!report.consistent);
        assert_eq!(report.missing_in_lock, vec!["token".to_string()]);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn lock_dependencies_records_local_and_git() {
        use crate::config::Dependency;
        let mut deps = HashMap::new();
        deps.insert(
            "math".to_string(),
            Dependency {
                path: Some("libs/math".to_string()),
                ..Default::default()
            },
        );
        deps.insert(
            "token".to_string(),
            Dependency {
                git: Some("https://github.com/org/token".to_string()),
                tag: Some("v1.2.0".to_string()),
                ..Default::default()
            },
        );

        let locked = lock_dependencies(&std::env::temp_dir(), &deps);
        assert_eq!(locked.len(), 2);

        let math = locked.iter().find(|d| d.name == "math").unwrap();
        assert_eq!(math.source, "local");
        assert!(math.git_url.is_empty());

        let token = locked.iter().find(|d| d.name == "token").unwrap();
        assert_eq!(token.source, "git");
        assert_eq!(token.git_url, "https://github.com/org/token");
        assert_eq!(token.resolved_reference, "v1.2.0");
        assert!(token.commit_sha.is_empty());

        // Round-trips through the lock file TOML (empty commit_sha omitted).
        let lock = LockFile {
            version: LOCK_VERSION,
            deploy_order: vec![],
            contracts: vec![],
            dependencies: locked,
        };
        let toml = lock_to_toml(&lock).unwrap();
        let parsed = lock_from_toml(&toml).unwrap();
        assert_eq!(parsed.dependencies, lock.dependencies);
    }

    #[test]
    fn dependency_lock_records_cache_and_integrity() {
        use crate::config::Dependency;
        let root = std::env::temp_dir().join("sdkt_lock_dep_full");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let mut deps = HashMap::new();
        deps.insert(
            "token".to_string(),
            Dependency {
                git: Some("https://github.com/org/token".to_string()),
                tag: Some("v1.2.0".to_string()),
                ..Default::default()
            },
        );
        deps.insert(
            "math".to_string(),
            Dependency {
                path: Some("libs/math".to_string()),
                ..Default::default()
            },
        );
        let locked = lock_dependencies(&root, &deps);
        let git = locked.iter().find(|d| d.name == "token").unwrap();
        assert_eq!(git.source, "git");
        assert_eq!(git.original_source, "https://github.com/org/token");
        assert_eq!(git.git_url, "https://github.com/org/token");
        assert_eq!(git.resolved_reference, "v1.2.0");

        let math = locked.iter().find(|d| d.name == "math").unwrap();
        assert_eq!(math.source, "local");
        assert_eq!(
            math.original_source,
            root.join("libs/math").to_string_lossy()
        );

        // Round-trip via lock file TOML, including new fields.
        let lock = LockFile {
            version: LOCK_VERSION,
            deploy_order: vec![],
            contracts: vec![],
            dependencies: locked,
        };
        let toml = lock_to_toml(&lock).unwrap();
        let parsed = lock_from_toml(&toml).unwrap();
        assert_eq!(parsed.dependencies, lock.dependencies);
    }

    #[test]
    fn verify_dependencies_consistent_when_matched() {
        let root = std::env::temp_dir().join("sdkt_lock_dep_verify_ok");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        // Local path dependency that exists on disk.
        let lp = root.join("libs/math");
        fs::create_dir_all(&lp).unwrap();
        fs::write(lp.join("lib.rs"), b"pub fn add(a:u32,b:u32)->u32{a+b}").unwrap();

        let mut deps = HashMap::new();
        deps.insert(
            "math".to_string(),
            Dependency {
                path: Some("libs/math".to_string()),
                ..Default::default()
            },
        );
        let config = config_with_deps(deps);

        // No lock yet -> MissingInLock for every dependency.
        let rep = verify_dependencies(&root, &config);
        assert!(!rep.present);
        assert!(!rep.consistent);
        assert_eq!(rep.checked, 1);
        assert_eq!(rep.mismatches.len(), 1);
        assert_eq!(rep.mismatches[0].kind, DepMismatchKind::MissingInLock);

        // Write a matching lock, then verification is consistent.
        let lock = LockFile {
            version: LOCK_VERSION,
            deploy_order: vec![],
            contracts: vec![],
            dependencies: lock_dependencies(&root, &config.dependencies),
        };
        write_lock(&root, &lock).unwrap();
        let rep = verify_dependencies(&root, &config);
        assert!(rep.present);
        assert!(rep.consistent);
        assert!(rep.mismatches.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_dependencies_detects_path_missing_and_drift() {
        let root = std::env::temp_dir().join("sdkt_lock_dep_verify_drift");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let lp = root.join("libs/math");
        fs::create_dir_all(&lp).unwrap();
        fs::write(lp.join("lib.rs"), b"x").unwrap();

        let mut deps = HashMap::new();
        deps.insert(
            "math".to_string(),
            Dependency {
                path: Some("libs/math".to_string()),
                ..Default::default()
            },
        );
        let config = config_with_deps(deps);
        let lock = LockFile {
            version: LOCK_VERSION,
            deploy_order: vec![],
            contracts: vec![],
            dependencies: lock_dependencies(&root, &config.dependencies),
        };
        write_lock(&root, &lock).unwrap();

        // Delete the path dependency -> PathMissing.
        let _ = fs::remove_dir_all(&lp);
        let rep = verify_dependencies(&root, &config);
        assert!(!rep.consistent);
        assert!(rep
            .mismatches
            .iter()
            .any(|m| m.kind == DepMismatchKind::PathMissing));

        // Restore; then change the manifest path -> SourceChanged.
        fs::create_dir_all(&lp).unwrap();
        let mut deps2 = HashMap::new();
        deps2.insert(
            "math".to_string(),
            Dependency {
                path: Some("libs/other".to_string()),
                ..Default::default()
            },
        );
        let config2 = config_with_deps(deps2);
        let rep = verify_dependencies(&root, &config2);
        assert!(!rep.consistent);
        assert!(rep
            .mismatches
            .iter()
            .any(|m| m.kind == DepMismatchKind::SourceChanged));

        // Lock records a dep absent from the manifest -> NotInManifest.
        let mut deps3 = HashMap::new();
        deps3.insert(
            "stale".to_string(),
            Dependency {
                path: Some("libs/stale".to_string()),
                ..Default::default()
            },
        );
        let config3 = config_with_deps(deps3);
        let rep = verify_dependencies(&root, &config3);
        assert!(!rep.consistent);
        assert!(rep
            .mismatches
            .iter()
            .any(|m| m.kind == DepMismatchKind::NotInManifest));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_dependencies_detects_git_cache_mismatch() {
        let root = std::env::temp_dir().join("sdkt_lock_dep_verify_git");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let mut deps = HashMap::new();
        deps.insert(
            "tok".to_string(),
            Dependency {
                git: Some("https://example.com/org/tok".to_string()),
                tag: Some("v2.0.0".to_string()),
                ..Default::default()
            },
        );
        let config = config_with_deps(deps);

        // Lock claims a resolved commit but cache is absent -> CacheMissing.
        let mut lock_dep = lock_dependencies(&root, &config.dependencies);
        lock_dep[0].commit_sha = "deadbeef".repeat(5); // 40 hex chars
        let lock = LockFile {
            version: LOCK_VERSION,
            deploy_order: vec![],
            contracts: vec![],
            dependencies: lock_dep,
        };
        write_lock(&root, &lock).unwrap();

        let rep = verify_dependencies(&root, &config);
        assert!(!rep.consistent);
        assert!(rep
            .mismatches
            .iter()
            .any(|m| m.kind == DepMismatchKind::CacheMissing));

        let _ = fs::remove_dir_all(&root);
    }

    // Helper: build a DevKitConfig with only package dependencies (no contracts),
    // reusing the existing minimal config builder.
    fn config_with_deps(dependencies: HashMap<String, Dependency>) -> DevKitConfig {
        DevKitConfig {
            network: NetworkConfig::default(),
            decode: DecodeConfig::default(),
            storage: StorageConfig::default(),
            contracts: HashMap::new(),
            dependencies,
            ..Default::default()
        }
    }
}
