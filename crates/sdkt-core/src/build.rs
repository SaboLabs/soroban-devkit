use crate::config::DevKitConfig;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub enum BuildError {
    MissingConfig,
    PathNotFound(String),
    CargoFailed { path: String, stderr: String },
    ArtifactNotFound(String),
    InvalidProject(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::MissingConfig => write!(f, "No [contracts] configured in .sdkt.toml"),
            BuildError::PathNotFound(path) => write!(f, "Contract path does not exist: {}", path),
            BuildError::CargoFailed { path, stderr } => {
                write!(f, "Cargo build failed in {}:\n{}", path, stderr)
            }
            BuildError::ArtifactNotFound(path) => {
                write!(f, "Expected WASM artifact not found at: {}", path)
            }
            BuildError::InvalidProject(msg) => {
                write!(f, "Invalid project dependency graph: {}", msg)
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// Result of building a single contract.
#[derive(Debug, PartialEq)]
pub struct BuildResult {
    pub alias: String,
    pub path: String,
    pub wasm_artifact: PathBuf,
}

/// Builds all contracts defined in the DevKitConfig.
///
/// For each contract, it navigates to the configured `path` and runs:
/// `cargo build --target wasm32-unknown-unknown --release`
///
/// The build proceeds in the dependency-resolved deploy order produced by
/// [`crate::project::resolve_deploy_order`], so a malformed graph
/// (unknown/self/duplicate dependency or a cycle) is rejected up front with a
/// clear error before any `cargo` invocation.
pub fn build_workspace(config: &DevKitConfig) -> Result<Vec<BuildResult>, BuildError> {
    if config.contracts.is_empty() {
        return Err(BuildError::MissingConfig);
    }

    // M34.2 — validate + order via the single shared resolver. This ensures
    // build, deploy, and lock generation all use the same resolved graph.
    let ordered = crate::project::resolve_deploy_order(config)
        .map_err(|e| BuildError::InvalidProject(e.to_string()))?;

    let mut results = Vec::new();

    for alias in &ordered {
        let contract_cfg = config
            .contracts
            .get(alias)
            .expect("alias from resolved order");
        let path = Path::new(&contract_cfg.path);

        if !path.exists() || !path.is_dir() {
            return Err(BuildError::PathNotFound(contract_cfg.path.clone()));
        }

        // Execute cargo build
        let output = Command::new("cargo")
            .arg("build")
            .arg("--target")
            .arg("wasm32-unknown-unknown")
            .arg("--release")
            .current_dir(path)
            .output()
            .map_err(|e| BuildError::CargoFailed {
                path: contract_cfg.path.clone(),
                stderr: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(BuildError::CargoFailed {
                path: contract_cfg.path.clone(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        // We assume a standard Soroban project structure where Cargo.toml has a package name.
        // For sdkt build, we will attempt to extract the expected artifact name from Cargo.toml,
        // or just glob the target/wasm32-unknown-unknown/release/*.wasm dir.
        // For stability without adding `cargo-metadata` dependency, we will look for any .wasm file
        // generated in the release directory.
        let target_dir = path
            .join("target")
            .join("wasm32-unknown-unknown")
            .join("release");

        let mut found_wasm = None;
        if target_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&target_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().is_some_and(|ext| ext == "wasm") {
                        found_wasm = Some(p);
                        break;
                    }
                }
            }
        }

        let wasm_artifact = found_wasm
            .ok_or_else(|| BuildError::ArtifactNotFound(target_dir.display().to_string()))?;

        results.push(BuildResult {
            alias: alias.clone(),
            path: contract_cfg.path.clone(),
            wasm_artifact,
        });
    }

    // M34.1 — generate `sdkt.lock` next to `.sdkt.toml` recording every built
    // artifact's SHA-256 and the deterministic deploy order. Advisory only:
    // if lock generation fails (e.g. an artifact vanished between build and
    // hashing), surface a warning but do not fail the build.
    if let Ok(lock) = crate::lock::generate_lock(Path::new("."), config) {
        match crate::lock::write_lock(Path::new("."), &lock) {
            Ok(path) => {
                if let Ok(toml) = crate::lock::lock_to_toml(&lock) {
                    println!("✓ Wrote {}", path.display());
                    println!("{}", toml);
                }
            }
            Err(e) => eprintln!("Warning: could not write sdkt.lock: {}", e),
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContractConfig;
    use std::collections::HashMap;

    #[test]
    fn test_build_empty_config() {
        let config = DevKitConfig::default();
        let res = build_workspace(&config);
        assert!(matches!(res, Err(BuildError::MissingConfig)));
    }

    #[test]
    fn test_build_missing_path() {
        let mut config = DevKitConfig::default();
        let mut contracts = HashMap::new();
        contracts.insert(
            "token".to_string(),
            ContractConfig {
                path: "does_not_exist_xyz".to_string(),
                deploy_after: vec![],
                depends_on: vec![],
            },
        );
        config.contracts = contracts;

        let res = build_workspace(&config);
        match res {
            Err(BuildError::PathNotFound(p)) => assert_eq!(p, "does_not_exist_xyz"),
            _ => panic!("Expected PathNotFound"),
        }
    }
}
