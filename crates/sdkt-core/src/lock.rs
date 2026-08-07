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

/// The on-disk `sdkt.lock` structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockFile {
    /// Schema version. Bump if the layout changes incompatibly.
    pub version: u32,
    /// Deterministic deploy order (alias list, index 0 deploys first).
    pub deploy_order: Vec<String>,
    /// Per-contract locked artifacts.
    pub contracts: Vec<LockEntry>,
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
    })
}

/// Serialize a [`LockFile`] to TOML.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContractConfig, DecodeConfig, DevKitConfig, NetworkConfig, StorageConfig};
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
}
