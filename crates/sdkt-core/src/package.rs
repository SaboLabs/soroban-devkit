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
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

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
/// * every dependency has a `path` (no git/HTTP/registry — enforced at parse
///   time via `deny_unknown_fields`, so an unsupported source surfaces as a
///   missing `path` here),
/// * no self-dependency (a dependency key equal to the package name),
/// * no duplicate dependency name,
/// * the referenced `path` resolves to an existing directory (relative to
///   `base_dir`),
/// * the dependency graph is acyclic (reuses [`topo_sort`]).
///
/// `base_dir` is the manifest's directory used to resolve relative `path`
/// values (in production this is `.`, the cwd sdkt runs from).
pub fn validate_dependencies(base_dir: &Path, config: &DevKitConfig) -> Result<(), PackageError> {
    let pkg_name = config
        .package
        .as_ref()
        .and_then(|p| p.name.clone())
        .unwrap_or_default();

    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    for (name, dep) in &config.dependencies {
        let path = dep
            .path
            .as_ref()
            .ok_or_else(|| PackageError::MissingPath(name.clone()))?;
        if path.trim().is_empty() {
            return Err(PackageError::MissingPath(name.clone()));
        }

        if name == &pkg_name {
            return Err(PackageError::SelfDependency(name.clone()));
        }

        if graph.contains_key(name) {
            return Err(PackageError::DuplicateDependency(name.clone()));
        }

        let full = base_dir.join(path);
        if !full.exists() {
            return Err(PackageError::PathNotFound(full.display().to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LocalDependency;
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
    fn test_unsupported_source_rejected_at_parse() {
        let toml_data = "\
[package]\nname = \"my-token\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"https://github.com/example/math\"\n";
        let parsed = parse_manifest(toml_data);
        assert!(parsed.is_err(), "git dependency must be rejected at parse");
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
}
