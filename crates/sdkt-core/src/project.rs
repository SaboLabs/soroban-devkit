use crate::config::DevKitConfig;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
pub enum ProjectError {
    MissingConfig,
    CircularDependency(String),
    UnknownDependency(String),
    SelfDependency(String),
    DuplicateDependency(String),
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
                "Contract specifies a dependency that is not defined in [contracts]: {}",
                dep
            ),
            ProjectError::SelfDependency(alias) => write!(
                f,
                "Contract '{}' lists itself as a dependency (self-dependency)",
                alias
            ),
            ProjectError::DuplicateDependency(alias) => write!(
                f,
                "Contract '{}' declares the same dependency more than once",
                alias
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

/// Resolves the deployment order based on the merged `deploy_after` +
/// `depends_on` dependency rules using a topological sort (Kahn's algorithm).
/// Returns the ordered list of contract aliases to deploy.
///
/// This is the single source of truth for dependency-graph resolution. It
/// validates the graph and returns a clear [`ProjectError`] on any problem:
/// self-dependency, duplicate dependency, unknown dependency, or a cycle.
/// Every consumer (build, deploy, lock generation) calls this, so they all
/// share the same validated, deterministic ordering.
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

    // Build the graph, validating as we go.
    for (alias, contract_cfg) in &config.contracts {
        // Merge both dependency spellings (M34.2: `depends_on` canonical,
        // `deploy_after` legacy). De-duplicate so a contract listed twice
        // does not inflate the in-degree.
        let mut merged: Vec<String> = Vec::new();
        for dep in contract_cfg
            .deploy_after
            .iter()
            .chain(contract_cfg.depends_on.iter())
        {
            if merged.contains(dep) {
                return Err(ProjectError::DuplicateDependency(alias.clone()));
            }
            // Self-dependency check.
            if dep == alias {
                return Err(ProjectError::SelfDependency(alias.clone()));
            }
            // Unknown dependency check.
            if !config.contracts.contains_key(dep) {
                return Err(ProjectError::UnknownDependency(dep.clone()));
            }
            merged.push(dep.clone());
        }

        // `dep` must be deployed before `alias`. So edge is dep -> alias.
        for dep in &merged {
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

/// Validate the dependency graph without resolving a concrete deploy order.
///
/// Convenience wrapper that reuses [`resolve_deploy_order`] (the single
/// topological-sort implementation) and discards the ordering. Returns a clear
/// error for any invalid graph (self-dependency, duplicate dependency, unknown
/// dependency, or cycle).
pub fn validate_project(config: &DevKitConfig) -> Result<(), ProjectError> {
    resolve_deploy_order(config).map(|_| ())
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

    fn cfg(pairs: Vec<(&str, Vec<String>, Vec<String>)>) -> DevKitConfig {
        let mut map = HashMap::new();
        for (alias, deploy_after, depends_on) in pairs {
            map.insert(
                alias.to_string(),
                ContractConfig {
                    path: format!("contracts/{}", alias),
                    deploy_after,
                    depends_on,
                },
            );
        }
        DevKitConfig {
            contracts: map,
            ..Default::default()
        }
    }

    #[test]
    fn test_missing_config() {
        let config = DevKitConfig::default();
        let res = resolve_deploy_order(&config);
        assert!(matches!(res, Err(ProjectError::MissingConfig)));
    }

    #[test]
    fn test_single_contract() {
        let config = cfg(vec![("token", vec![], vec![])]);
        let res = resolve_deploy_order(&config).unwrap();
        assert_eq!(res, vec!["token".to_string()]);
    }

    #[test]
    fn test_multiple_with_dependencies() {
        let config = cfg(vec![
            (
                "router",
                vec!["token".to_string(), "amm".to_string()],
                vec![],
            ),
            ("token", vec![], vec![]),
            ("amm", vec!["token".to_string()], vec![]),
        ]);
        let res = resolve_deploy_order(&config).unwrap();
        assert_eq!(
            res,
            vec!["token".to_string(), "amm".to_string(), "router".to_string()]
        );
    }

    #[test]
    fn test_depends_on_field() {
        // M34.2 canonical field used instead of deploy_after.
        let config = cfg(vec![
            ("router", vec![], vec!["token".to_string()]),
            ("token", vec![], vec![]),
        ]);
        let res = resolve_deploy_order(&config).unwrap();
        assert_eq!(res, vec!["token".to_string(), "router".to_string()]);
    }

    #[test]
    fn test_merged_deploy_after_and_depends_on() {
        // A contract may split its dependencies across both spellings.
        let config = cfg(vec![
            ("router", vec!["token".to_string()], vec!["amm".to_string()]),
            ("token", vec![], vec![]),
            ("amm", vec![], vec!["token".to_string()]),
        ]);
        let res = resolve_deploy_order(&config).unwrap();
        assert_eq!(
            res,
            vec!["token".to_string(), "amm".to_string(), "router".to_string()]
        );
    }

    #[test]
    fn test_unknown_dependency() {
        let config = cfg(vec![("router", vec!["does_not_exist".to_string()], vec![])]);
        match resolve_deploy_order(&config) {
            Err(ProjectError::UnknownDependency(d)) => assert_eq!(d, "does_not_exist"),
            _ => panic!("Expected UnknownDependency"),
        }
    }

    #[test]
    fn test_self_dependency() {
        let config = cfg(vec![("token", vec!["token".to_string()], vec![])]);
        match resolve_deploy_order(&config) {
            Err(ProjectError::SelfDependency(a)) => assert_eq!(a, "token"),
            _ => panic!("Expected SelfDependency"),
        }
    }

    #[test]
    fn test_duplicate_dependency() {
        // Same dependency declared twice (once per spelling) -> duplicate.
        let config = cfg(vec![
            (
                "router",
                vec!["token".to_string()],
                vec!["token".to_string()],
            ),
            ("token", vec![], vec![]),
        ]);
        match resolve_deploy_order(&config) {
            Err(ProjectError::DuplicateDependency(a)) => assert_eq!(a, "router"),
            other => panic!("Expected DuplicateDependency, got {:?}", other),
        }
    }

    #[test]
    fn test_circular_dependency() {
        let config = cfg(vec![
            ("a", vec!["b".to_string()], vec![]),
            ("b", vec!["a".to_string()], vec![]),
        ]);
        match resolve_deploy_order(&config) {
            Err(ProjectError::CircularDependency(d)) => assert!(d == "a" || d == "b"),
            _ => panic!("Expected CircularDependency"),
        }
    }

    #[test]
    fn test_deterministic_ordering() {
        // Diamond: d -> {b, c}, b -> a, c -> a. Order must be stable.
        let config = cfg(vec![
            ("d", vec!["b".to_string(), "c".to_string()], vec![]),
            ("b", vec!["a".to_string()], vec![]),
            ("c", vec!["a".to_string()], vec![]),
            ("a", vec![], vec![]),
        ]);
        let first = resolve_deploy_order(&config).unwrap();
        for _ in 0..20 {
            assert_eq!(resolve_deploy_order(&config).unwrap(), first);
        }
        // a before b and c; b and c before d.
        let pos = |s: &str| first.iter().position(|x| x == s).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn test_validate_project_ok_and_err() {
        let ok = cfg(vec![
            ("router", vec![], vec!["token".to_string()]),
            ("token", vec![], vec![]),
        ]);
        assert!(validate_project(&ok).is_ok());

        let bad = cfg(vec![("token", vec!["token".to_string()], vec![])]);
        assert!(matches!(
            validate_project(&bad),
            Err(ProjectError::SelfDependency(_))
        ));
    }
}
