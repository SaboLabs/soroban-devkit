use crate::config::DevKitConfig;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
pub enum ProjectError {
    MissingConfig,
    CircularDependency(String),
    UnknownDependency(String),
    MissingArtifact(String),
    BuildRequired(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::MissingConfig => write!(f, "No [contracts] configured in .sdkt.toml"),
            ProjectError::CircularDependency(path) => write!(
                f,
                "Circular dependency detected in contract deployment graph: {}",
                path
            ),
            ProjectError::UnknownDependency(dep) => write!(
                f,
                "Contract specifies a deploy_after dependency that is not defined: {}",
                dep
            ),
            ProjectError::MissingArtifact(path) => {
                write!(f, "Expected WASM artifact not found at: {}", path)
            }
            ProjectError::BuildRequired(alias) => write!(
                f,
                "WASM artifact missing for contract '{}'. Run `sdkt build` first.",
                alias
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

/// A resolved contract ready for deployment.
#[derive(Debug, PartialEq)]
pub struct ResolvedContract {
    pub alias: String,
    pub path: String,
    pub wasm_artifact: PathBuf,
}

/// Resolves the deployment order based on `deploy_after` rules using a topological sort.
/// Returns the ordered list of contract aliases to deploy.
pub fn resolve_deploy_order(config: &DevKitConfig) -> Result<Vec<String>, ProjectError> {
    if config.contracts.is_empty() {
        return Err(ProjectError::MissingConfig);
    }

    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut adj_list: HashMap<String, Vec<String>> = HashMap::new();

    // Initialize tracking structures
    for alias in config.contracts.keys() {
        in_degree.insert(alias.clone(), 0);
        adj_list.insert(alias.clone(), Vec::new());
    }

    // Build the graph
    for (alias, contract_cfg) in &config.contracts {
        for dep in &contract_cfg.deploy_after {
            if !config.contracts.contains_key(dep) {
                return Err(ProjectError::UnknownDependency(dep.clone()));
            }
            // `dep` must be deployed before `alias`. So edge is dep -> alias.
            adj_list.get_mut(dep).unwrap().push(alias.clone());
            *in_degree.get_mut(alias).unwrap() += 1;
        }
    }

    // Kahn's algorithm for topological sort
    let mut queue: Vec<String> = Vec::new();
    // Enqueue nodes with no dependencies (in-degree 0)
    for (alias, &degree) in &in_degree {
        if degree == 0 {
            queue.push(alias.clone());
        }
    }

    // Sort queue initially to ensure deterministic ordering for nodes at the same tier
    queue.sort();

    let mut ordered = Vec::new();

    while !queue.is_empty() {
        // Sort queue at each step to maintain strict deterministic output regardless of HashMap iteration order.
        queue.sort();
        let current = queue.remove(0);
        ordered.push(current.clone());

        if let Some(neighbors) = adj_list.get(&current) {
            for neighbor in neighbors {
                if let Some(degree) = in_degree.get_mut(neighbor) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push(neighbor.clone());
                    }
                }
            }
        }
    }

    if ordered.len() != config.contracts.len() {
        // We have a cycle. Find one of the nodes that couldn't be resolved.
        for (alias, &degree) in &in_degree {
            if degree > 0 {
                return Err(ProjectError::CircularDependency(alias.clone()));
            }
        }
        return Err(ProjectError::CircularDependency("Unknown".to_string()));
    }

    Ok(ordered)
}

/// Resolves the deployment sequence and validates that all required WASM artifacts exist.
/// Does NOT perform actual deployment (which requires sdkt-rpc).
pub fn resolve_project(config: &DevKitConfig) -> Result<Vec<ResolvedContract>, ProjectError> {
    let ordered_aliases = resolve_deploy_order(config)?;
    let mut resolved = Vec::new();

    for alias in ordered_aliases {
        let cfg = config.contracts.get(&alias).unwrap();
        let path = Path::new(&cfg.path);

        // Attempt to locate the WASM artifact in the standard target directory
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
                        // For determinism, if multiple exist, pick the first (or ideally, we'd enforce one).
                        found_wasm = Some(p);
                        break;
                    }
                }
            }
        }

        let wasm_artifact = found_wasm.ok_or_else(|| ProjectError::BuildRequired(alias.clone()))?;

        resolved.push(ResolvedContract {
            alias,
            path: cfg.path.clone(),
            wasm_artifact,
        });
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContractConfig, DevKitConfig};
    use std::collections::HashMap;

    #[test]
    fn test_missing_config() {
        let config = DevKitConfig::default();
        let res = resolve_deploy_order(&config);
        assert!(matches!(res, Err(ProjectError::MissingConfig)));
    }

    #[test]
    fn test_single_contract() {
        let mut config = DevKitConfig::default();
        let mut map = HashMap::new();
        map.insert(
            "token".to_string(),
            ContractConfig {
                path: "contracts/token".to_string(),
                deploy_after: vec![],
            },
        );
        config.contracts = map;

        let res = resolve_deploy_order(&config).unwrap();
        assert_eq!(res, vec!["token".to_string()]);
    }

    #[test]
    fn test_multiple_with_dependencies() {
        let mut config = DevKitConfig::default();
        let mut map = HashMap::new();
        map.insert(
            "router".to_string(),
            ContractConfig {
                path: "contracts/router".to_string(),
                deploy_after: vec!["token".to_string(), "amm".to_string()],
            },
        );
        map.insert(
            "token".to_string(),
            ContractConfig {
                path: "contracts/token".to_string(),
                deploy_after: vec![],
            },
        );
        map.insert(
            "amm".to_string(),
            ContractConfig {
                path: "contracts/amm".to_string(),
                deploy_after: vec!["token".to_string()],
            },
        );
        config.contracts = map;

        let res = resolve_deploy_order(&config).unwrap();
        // token has 0 deps.
        // amm depends on token.
        // router depends on token and amm.
        assert_eq!(
            res,
            vec!["token".to_string(), "amm".to_string(), "router".to_string()]
        );
    }

    #[test]
    fn test_unknown_dependency() {
        let mut config = DevKitConfig::default();
        let mut map = HashMap::new();
        map.insert(
            "router".to_string(),
            ContractConfig {
                path: "contracts/router".to_string(),
                deploy_after: vec!["does_not_exist".to_string()],
            },
        );
        config.contracts = map;

        let res = resolve_deploy_order(&config);
        match res {
            Err(ProjectError::UnknownDependency(d)) => assert_eq!(d, "does_not_exist"),
            _ => panic!("Expected UnknownDependency"),
        }
    }

    #[test]
    fn test_circular_dependency() {
        let mut config = DevKitConfig::default();
        let mut map = HashMap::new();
        map.insert(
            "a".to_string(),
            ContractConfig {
                path: "contracts/a".to_string(),
                deploy_after: vec!["b".to_string()],
            },
        );
        map.insert(
            "b".to_string(),
            ContractConfig {
                path: "contracts/b".to_string(),
                deploy_after: vec!["a".to_string()],
            },
        );
        config.contracts = map;

        let res = resolve_deploy_order(&config);
        match res {
            Err(ProjectError::CircularDependency(d)) => assert!(d == "a" || d == "b"),
            _ => panic!("Expected CircularDependency"),
        }
    }
}
