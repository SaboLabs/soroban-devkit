//! Core configuration engine for the Soroban DevKit (sdkt).
//!
//! This module defines the global configuration structures and handles
//! the loading of configs from files, environment variables, and CLI overrides.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// High-level project-wide configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DevKitConfig {
    /// Soroban network settings.
    #[serde(default)]
    pub network: NetworkConfig,
    /// XDR decoder configurations.
    #[serde(default)]
    pub decode: DecodeConfig,
    /// Storage inspection settings.
    #[serde(default)]
    pub storage: StorageConfig,
    /// Workspace contract configuration mapping.
    #[serde(default)]
    pub contracts: std::collections::HashMap<String, ContractConfig>,
    /// Optional package manifest metadata (). Present when this project
    /// is itself a publishable package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageConfig>,
    /// Local and Git package dependencies ( / ). Keys are package
    /// names; values describe exactly one source. A dependency is either a
    /// local `path` reference or a `git` reference (with exactly one of
    /// `tag` / `branch` / `rev`). No registry, no HTTP crate sources.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub dependencies: std::collections::HashMap<String, Dependency>,
}

/// Settings for a specific contract in a workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractConfig {
    /// Path to the contract source directory (where Cargo.toml resides).
    pub path: String,
    /// Optional list of contract aliases that must be deployed before this one.
    /// (`deploy_after` is the original spelling; `depends_on` is the canonical
    /// 4.2 field. Both are accepted and merged during resolution.)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deploy_after: Vec<String>,
    /// Canonical 4.2 dependency declaration. Semantically identical to
    /// `deploy_after`; listed here for explicit package-dependency graphs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Constructor arguments passed to the contract at create time.
    /// Each string is a TYPE:VALUE pair (e.g. "u32:100", "address:G...",
    /// "string:hello") or a base64-encoded ScVal. Same format as `sdkt deploy --arg`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ctor_args: Vec<String>,
}

/// Package manifest metadata ().
///
/// Optional `[package]` section that marks a project as a reusable,
/// publishable package. All fields are optional at parse time so that a
/// partial/invalid manifest still deserializes; `validate_package` enforces
/// the required fields (name, version) and version format with clear errors.
/// Unknown fields are allowed (forward-compatible with future manifest keys
/// such as `authors`, `license`, `repository`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PackageConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A Git reference selector for a [`Dependency`] ().
///
/// Exactly one variant must be set. `Tag` pins to a tag, `Branch` to a branch
/// head, `Rev` to an exact commit SHA. This mirrors Cargo's git dependency
/// reference semantics (minus `default-features`/subpath concerns outside the
/// scope of ).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitReference {
    /// A specific tag, e.g. `v1.2.0`.
    Tag(String),
    /// A branch head, e.g. `main`.
    Branch(String),
    /// An exact commit SHA.
    Rev(String),
}

/// A single package dependency ( / ).
///
/// Exactly one source is allowed: a local `path` OR a `git` URL. When `git`
/// is set, exactly one of `tag` / `branch` / `rev` must also be set (the
/// [`crate::package::validate_dependencies`] resolver rejects any malformed
/// combination). `deny_unknown_fields` rejects unrecognized keys so future
/// source kinds (e.g. a registry) are explicit additions, not silent parses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    /// Local filesystem path (relative to the depending manifest) to the
    /// package root. Mutually exclusive with [`Dependency::git`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Git remote URL. Mutually exclusive with [`Dependency::path`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Git tag reference (e.g. `v1.2.0`). Mutually exclusive with `branch`/`rev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Git branch reference (e.g. `main`). Mutually exclusive with `tag`/`rev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Exact Git commit SHA. Mutually exclusive with `tag`/`branch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Optional semver version constraint for `git` deps resolved against the
    /// remote's tags (). When set without `tag`/`branch`/`rev`, `sdkt package
    /// update` selects the highest tag satisfying the constraint. An explicit
    /// `tag`/`branch`/`rev` always takes precedence and the constraint is
    /// ignored (mirrors how `rev`/`path` deps bypass version resolution).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Backward-compatibility alias for the local-only dependency type.
///
/// `LocalDependency` previously held a single `path` field; it is now a thin
/// type alias of [`Dependency`] so existing code/tests keep compiling.
pub type LocalDependency = Dependency;

/// Soroban network connection settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkConfig {
    /// Target Soroban RPC URL.
    pub rpc_url: String,
    /// Core passphrase matching target network.
    pub passphrase: String,
    /// Optional timeout for RPC requests in seconds (default: 15).
    pub timeout_secs: Option<u64>,
    /// Optional maximum concurrent connections in the pool (default: 100).
    pub pool_max_idle_per_host: Option<usize>,
}

/// Settings modifying XDR decoding actions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecodeConfig {
    /// Limit max depth during nested XDR parsing. `0` means unlimited
    /// (`Limits::none` in the XDR layer) — same semantics as the decoder.
    #[serde(default = "default_decode_max_depth")]
    pub max_depth: u32,
    /// Fallback to hex decoding if base64 detection fails.
    #[serde(default = "default_allow_fallback_hex")]
    pub allow_fallback_hex: bool,
}

/// Settings for Soroban storage inspection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageConfig {
    /// Maximum number of storage entries to fetch per page.
    #[serde(default = "default_storage_max_entries")]
    pub max_entries: usize,
    /// Number of days before TTL expiration to start warning.
    #[serde(default = "default_ttl_warning_days")]
    pub ttl_warning_days: u32,
}

fn default_decode_max_depth() -> u32 {
    500
}

fn default_allow_fallback_hex() -> bool {
    true
}

fn default_storage_max_entries() -> usize {
    200
}

fn default_ttl_warning_days() -> u32 {
    30
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            max_entries: 200,
            ttl_warning_days: 30,
        }
    }
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            max_depth: 500,
            allow_fallback_hex: true,
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            rpc_url: "https://soroban-testnet.stellar.org".to_string(),
            passphrase: "Test SDF Network ; September 2015".to_string(),
            timeout_secs: Some(15),
            pool_max_idle_per_host: Some(100),
        }
    }
}


/// A single located configuration validation problem (#565).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigProblem {
    pub section: String,
    pub key: String,
    pub message: String,
}

/// Stage 1 schema validation on the raw `.sdkt.toml` text (#565).
///
/// Parses `content` as `toml::Value` and checks for unknown keys before serde
/// deserialization can discard them:
/// - Top-level keys: `network`, `decode`, `storage`, `contracts`, `package`, `dependencies`.
/// - `[network]` keys: `rpc_url`, `passphrase`, `timeout_secs`, `pool_max_idle_per_host`.
/// - `[decode]` keys: `max_depth`, `allow_fallback_hex`.
/// - `[storage]` keys: `max_entries`, `ttl_warning_days`.
/// - `[contracts.<alias>]` keys: `path` (required), `deploy_after`, `depends_on`, `ctor_args`.
/// - `[dependencies.<name>]` keys: `path`, `git`, `tag`, `branch`, `rev`, `version`.
/// - `[package]` is exempt: unknown keys are allowed by design.
pub fn validate_config_schema(content: &str) -> Vec<ConfigProblem> {
    let val: toml::Value = match toml::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            return vec![ConfigProblem {
                section: ".sdkt.toml".to_string(),
                key: "parse".to_string(),
                message: format!("TOML parse error: {e}"),
            }];
        }
    };

    let Some(root) = val.as_table() else {
        return vec![ConfigProblem {
            section: ".sdkt.toml".to_string(),
            key: "root".to_string(),
            message: "expected top-level TOML table".to_string(),
        }];
    };

    let mut problems = Vec::new();
    const TOP_KEYS: &[&str] = &[
        "network",
        "decode",
        "storage",
        "contracts",
        "package",
        "dependencies",
    ];
    const NETWORK_KEYS: &[&str] = &[
        "rpc_url",
        "passphrase",
        "timeout_secs",
        "pool_max_idle_per_host",
    ];
    const DECODE_KEYS: &[&str] = &["max_depth", "allow_fallback_hex"];
    const STORAGE_KEYS: &[&str] = &["max_entries", "ttl_warning_days"];
    const CONTRACT_KEYS: &[&str] = &["path", "deploy_after", "depends_on", "ctor_args"];
    const DEP_KEYS: &[&str] = &["path", "git", "tag", "branch", "rev", "version"];

    for (k, v) in root {
        if !TOP_KEYS.contains(&k.as_str()) {
            problems.push(ConfigProblem {
                section: "root".to_string(),
                key: k.clone(),
                message: format!("unknown top-level key or section '{k}'"),
            });
            continue;
        }

        match k.as_str() {
            "network" => {
                if let Some(tbl) = v.as_table() {
                    for nk in tbl.keys() {
                        if !NETWORK_KEYS.contains(&nk.as_str()) {
                            problems.push(ConfigProblem {
                                section: "network".to_string(),
                                key: nk.clone(),
                                message: format!("unknown key '{nk}' in [network]"),
                            });
                        }
                    }
                }
            }
            "decode" => {
                if let Some(tbl) = v.as_table() {
                    for dk in tbl.keys() {
                        if !DECODE_KEYS.contains(&dk.as_str()) {
                            problems.push(ConfigProblem {
                                section: "decode".to_string(),
                                key: dk.clone(),
                                message: format!("unknown key '{dk}' in [decode]"),
                            });
                        }
                    }
                }
            }
            "storage" => {
                if let Some(tbl) = v.as_table() {
                    for sk in tbl.keys() {
                        if !STORAGE_KEYS.contains(&sk.as_str()) {
                            problems.push(ConfigProblem {
                                section: "storage".to_string(),
                                key: sk.clone(),
                                message: format!("unknown key '{sk}' in [storage]"),
                            });
                        }
                    }
                }
            }
            "contracts" => {
                if let Some(contracts_tbl) = v.as_table() {
                    for (alias, entry) in contracts_tbl {
                        if let Some(ctbl) = entry.as_table() {
                            if !ctbl.contains_key("path") {
                                problems.push(ConfigProblem {
                                    section: format!("contracts.{alias}"),
                                    key: "path".to_string(),
                                    message: format!(
                                        "contract '{alias}' is missing required key 'path'"
                                    ),
                                });
                            }
                            for ck in ctbl.keys() {
                                if !CONTRACT_KEYS.contains(&ck.as_str()) {
                                    problems.push(ConfigProblem {
                                        section: format!("contracts.{alias}"),
                                        key: ck.clone(),
                                        message: format!(
                                            "unknown key '{ck}' in [contracts.{alias}]"
                                        ),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            "dependencies" => {
                if let Some(deps_tbl) = v.as_table() {
                    for (dep_name, entry) in deps_tbl {
                        if let Some(dtbl) = entry.as_table() {
                            for dk in dtbl.keys() {
                                if !DEP_KEYS.contains(&dk.as_str()) {
                                    problems.push(ConfigProblem {
                                        section: format!("dependencies.{dep_name}"),
                                        key: dk.clone(),
                                        message: format!(
                                            "unknown key '{dk}' in [dependencies.{dep_name}]"
                                        ),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            "package" => {
                // [package] is exempt: unknown keys are allowed by design.
            }
            _ => {}
        }
    }

    problems
}

impl DevKitConfig {
    /// Loads configuration from a TOML string.
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Validates semantic rules on the parsed configuration (#565):
    /// - Numeric ranges (`storage.max_entries > 0`, `network.timeout_secs > 0`,
    ///   `network.pool_max_idle_per_host > 0`; `decode.max_depth = 0` is valid
    ///   and means "unlimited").
    /// - Contract paths (`[contracts.<alias>].path` resolved against `base_dir`
    ///   must exist on disk and contain a `Cargo.toml`).
    /// - Dependency combinations (`validate_dependencies(base_dir, self)`).
    /// - Deploy order graph (`validate_project(self)` when `contracts` is non-empty).
    pub fn validate(&self, base_dir: &Path) -> Result<(), Vec<ConfigProblem>> {
        let mut problems = Vec::new();

        // 1. Numeric ranges
        if self.storage.max_entries == 0 {
            problems.push(ConfigProblem {
                section: "storage".to_string(),
                key: "max_entries".to_string(),
                message: "storage.max_entries must be greater than 0".to_string(),
            });
        }
        if let Some(0) = self.network.timeout_secs {
            problems.push(ConfigProblem {
                section: "network".to_string(),
                key: "timeout_secs".to_string(),
                message: "network.timeout_secs must be greater than 0".to_string(),
            });
        }
        if let Some(0) = self.network.pool_max_idle_per_host {
            problems.push(ConfigProblem {
                section: "network".to_string(),
                key: "pool_max_idle_per_host".to_string(),
                message: "network.pool_max_idle_per_host must be greater than 0".to_string(),
            });
        }

        // 2. Contract paths (sorted by alias for deterministic reporting)
        let mut aliases: Vec<&String> = self.contracts.keys().collect();
        aliases.sort();
        for alias in aliases {
            let cfg = &self.contracts[alias];
            let p = Path::new(&cfg.path);
            let resolved = if p.is_absolute() {
                p.to_path_buf()
            } else {
                base_dir.join(p)
            };
            if !resolved.exists() {
                problems.push(ConfigProblem {
                    section: format!("contracts.{alias}"),
                    key: "path".to_string(),
                    message: format!(
                        "contract '{alias}' path does not exist: {}",
                        resolved.display()
                    ),
                });
            } else if !resolved.join("Cargo.toml").exists() {
                problems.push(ConfigProblem {
                    section: format!("contracts.{alias}"),
                    key: "path".to_string(),
                    message: format!(
                        "contract '{alias}' path does not contain Cargo.toml: {}",
                        resolved.display()
                    ),
                });
            }
        }

        // 3. Dependency combinations
        if let Err(err) = crate::package::validate_dependencies(base_dir, self) {
            use crate::package::PackageError;
            let (key, message) = match &err {
                PackageError::MissingPath(n)
                | PackageError::UnsupportedSource(n)
                | PackageError::SelfDependency(n)
                | PackageError::CircularDependency(n)
                | PackageError::DuplicateDependency(n)
                | PackageError::MixedSources(n)
                | PackageError::MissingGitUrl(n)
                | PackageError::MultipleGitRefs(n)
                | PackageError::MissingGitRef(n)
                | PackageError::EmptyGitRef(n)
                | PackageError::DependencyNotCached(n) => (n.clone(), err.to_string()),
                PackageError::PathNotFound(_) => ("path".to_string(), err.to_string()),
                PackageError::InvalidVersion(_) => ("version".to_string(), err.to_string()),
                PackageError::InvalidGitUrl(_) | PackageError::UnsupportedUrlScheme(_) => {
                    ("git".to_string(), err.to_string())
                }
                _ => ("dependencies".to_string(), err.to_string()),
            };
            problems.push(ConfigProblem {
                section: "dependencies".to_string(),
                key,
                message,
            });
        }

        // 4. Deploy order (only when contracts is non-empty)
        if !self.contracts.is_empty() {
            if let Err(err) = crate::project::validate_project(self) {
                use crate::project::ProjectError;
                let key = match &err {
                    ProjectError::CircularDependency(k)
                    | ProjectError::UnknownDependency(k)
                    | ProjectError::SelfDependency(k)
                    | ProjectError::DuplicateDependency(k)
                    | ProjectError::BuildRequired(k) => k.clone(),
                    _ => "contracts".to_string(),
                };
                problems.push(ConfigProblem {
                    section: "contracts".to_string(),
                    key,
                    message: err.to_string(),
                });
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }

    /// Loads configuration from a specific file path.
    ///
    /// If the file does not exist, it falls back to the default configuration.
    /// A present-but-unparseable file (e.g. a duplicate contract name or
    /// malformed TOML) returns the underlying parse error so callers can
    /// surface a clear, human-readable message instead of silently falling
    /// back to an empty config.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.as_ref();
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let config = Self::from_toml(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let default_config = DevKitConfig::default();
        assert_eq!(default_config.decode.max_depth, 500);
        assert!(default_config.decode.allow_fallback_hex);
        assert_eq!(default_config.storage.max_entries, 200);
        assert_eq!(default_config.storage.ttl_warning_days, 30);
    }

    #[test]
    fn test_decode_storage_sections_parse_and_are_honoured() {
        // Issue #375: the [decode]/[storage] sections must reach the code paths
        // that consume them (XDR decode limits, storage paging + TTL warning).
        // This test asserts the config surface the wired code reads.
        let toml_data = r#"
            [decode]
            max_depth = 4
            allow_fallback_hex = false

            [storage]
            max_entries = 50
            ttl_warning_days = 7
        "#;
        let parsed = DevKitConfig::from_toml(toml_data).unwrap();
        assert_eq!(parsed.decode.max_depth, 4);
        assert!(!parsed.decode.allow_fallback_hex);
        assert_eq!(parsed.storage.max_entries, 50);
        assert_eq!(parsed.storage.ttl_warning_days, 7);
    }

    #[test]
    fn test_partial_decode_and_storage_sections_use_defaults() {
        // A section may set only one of its fields; the rest must fall back to
        // the documented defaults rather than failing to parse.
        let parsed = DevKitConfig::from_toml("[decode]\nmax_depth = 4\n").unwrap();
        assert_eq!(parsed.decode.max_depth, 4);
        assert!(parsed.decode.allow_fallback_hex);

        let parsed = DevKitConfig::from_toml("[storage]\nttl_warning_days = 7\n").unwrap();
        assert_eq!(parsed.storage.ttl_warning_days, 7);
        assert_eq!(parsed.storage.max_entries, 200);
    }

    #[test]
    fn test_default_ttl_warning_days_is_thirty() {
        // Regression (review round 3): wiring [storage] into the analyzer must
        // not silently change the default warning threshold. 30 days is the
        // pre-existing default and must stay the default.
        assert_eq!(DevKitConfig::default().storage.ttl_warning_days, 30);
        // An omitted section must resolve to the same value.
        let parsed = DevKitConfig::from_toml("[storage]\nmax_entries = 50\n").unwrap();
        assert_eq!(parsed.storage.ttl_warning_days, 30);
    }

    #[test]
    fn test_validate_accepts_zero_max_depth_as_unlimited() {
        // Issue #375: `max_depth = 0` is documented and implemented as
        // "unlimited" by decode_with_config, so the config validator must not
        // contradict it. A .sdkt.toml with max_depth = 0 must load and decode.
        let mut config = DevKitConfig::default();
        config.decode.max_depth = 0;
        assert!(
            config.validate(Path::new(".")).is_ok(),
            "max_depth = 0 must be valid (unlimited), matching decode_with_config"
        );
    }

    #[test]
    fn test_validate_accepts_positive_max_depth() {
        let mut config = DevKitConfig::default();
        config.decode.max_depth = 1;
        assert!(config.validate(Path::new(".")).is_ok());
        config.decode.max_depth = u32::MAX;
        assert!(config.validate(Path::new(".")).is_ok());
    }

    #[test]
    fn test_parse_valid_toml() {
        let toml_data = r#"
            [network]
            rpc_url = "http://localhost:8000"
            passphrase = "Standalone Network"

            [decode]
            max_depth = 16
            allow_fallback_hex = false

            [storage]
            max_entries = 100
            ttl_warning_days = 7
        "#;
        let parsed = DevKitConfig::from_toml(toml_data).unwrap();
        assert_eq!(parsed.network.rpc_url, "http://localhost:8000");
        assert_eq!(parsed.decode.max_depth, 16);
        assert!(!parsed.decode.allow_fallback_hex);
        assert_eq!(parsed.storage.max_entries, 100);
        assert_eq!(parsed.storage.ttl_warning_days, 7);
    }

    #[test]
    fn test_old_toml_without_storage() {
        let toml_data = r#"
            [network]
            rpc_url = "http://localhost:8000"
            passphrase = "Standalone Network"

            [decode]
            max_depth = 16
            allow_fallback_hex = false
        "#;
        let parsed = DevKitConfig::from_toml(toml_data).unwrap();
        assert_eq!(parsed.storage.max_entries, 200);
    }

    #[test]
    fn test_duplicate_contract_name_rejected() {
        // Duplicate `[contracts.token]` tables are a duplicate-name error.
        // toml rejects duplicate keys, so parsing must fail (the CLI then
        // surfaces a clear "duplicate key" message instead of silently
        // defaulting to an empty config).
        let toml_data = r#"
            [contracts.token]
            path = "contracts/token"

            [contracts.token]
            path = "contracts/token2"
        "#;
        let parsed = DevKitConfig::from_toml(toml_data);
        assert!(
            parsed.is_err(),
            "duplicate contract name must fail to parse"
        );
    }

    #[test]
    fn test_depends_on_field_parses() {
        let toml_data = r#"
            [contracts.token]
            path = "contracts/token"

            [contracts.router]
            path = "contracts/router"
            depends_on = ["token"]
        "#;
        let parsed = DevKitConfig::from_toml(toml_data).unwrap();
        assert_eq!(
            parsed.contracts.get("router").unwrap().depends_on,
            vec!["token".to_string()]
        );
        assert!(parsed
            .contracts
            .get("router")
            .unwrap()
            .deploy_after
            .is_empty());
    }

    #[test]
    fn test_ctor_args_field_parses() {
        let toml_data = r#"
            [contracts.token]
            path = "contracts/token"
            ctor_args = ["address:GABC123", "u32:100"]

            [contracts.vault]
            path = "contracts/vault"
            depends_on = ["token"]
        "#;
        let parsed = DevKitConfig::from_toml(toml_data).unwrap();
        let token = parsed.contracts.get("token").unwrap();
        assert_eq!(token.ctor_args.len(), 2);
        assert_eq!(token.ctor_args[0], "address:GABC123");
        assert_eq!(token.ctor_args[1], "u32:100");

        let vault = parsed.contracts.get("vault").unwrap();
        assert!(vault.ctor_args.is_empty());
    }

    #[test]
    fn test_empty_ctor_args_omitted_from_serialization() {
        let config = ContractConfig {
            path: "contracts/test".into(),
            deploy_after: vec![],
            depends_on: vec![],
            ctor_args: vec![],
        };
        let serialized = toml::to_string(&config).unwrap();
        assert!(!serialized.contains("ctor_args"));
    }

    // -----------------------------------------------------------------------
    // Issue #565: Stage 1 schema + Stage 2 semantic validation unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_schema_unknown_top_level_key_reported() {
        let problems = validate_config_schema("[netwrok]\nrpc_url = \"http://localhost:8000\"\n");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].section, "root");
        assert_eq!(problems[0].key, "netwrok");
    }

    #[test]
    fn test_schema_unknown_decode_key_reported() {
        let problems = validate_config_schema("[decode]\nmax_depth = 10\nunknown_flag = true\n");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].section, "decode");
        assert_eq!(problems[0].key, "unknown_flag");
    }

    #[test]
    fn test_schema_unknown_contract_key_and_missing_path_reported() {
        let problems = validate_config_schema("[contracts.token]\npath = \"contracts/token\"\nextra = 1\n");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].section, "contracts.token");
        assert_eq!(problems[0].key, "extra");

        let missing_path = validate_config_schema("[contracts.token]\ndepends_on = []\n");
        assert_eq!(missing_path.len(), 1);
        assert_eq!(missing_path[0].section, "contracts.token");
        assert_eq!(missing_path[0].key, "path");
    }

    #[test]
    fn test_schema_unknown_package_key_is_exempt() {
        let problems = validate_config_schema(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nfuture_key = \"allowed\"\n",
        );
        assert!(problems.is_empty(), "[package] unknown keys must be allowed");
    }

    #[test]
    fn test_schema_malformed_toml_and_duplicate_contract_table() {
        let malformed = validate_config_schema("[decode\nmax_depth = 10");
        assert_eq!(malformed.len(), 1);
        assert_eq!(malformed[0].section, ".sdkt.toml");
        assert_eq!(malformed[0].key, "parse");

        let dup = validate_config_schema(
            "[contracts.x]\npath = \"a\"\n[contracts.x]\npath = \"b\"\n",
        );
        assert_eq!(dup.len(), 1);
        assert_eq!(dup[0].section, ".sdkt.toml");
        assert_eq!(dup[0].key, "parse");
    }

    #[test]
    fn test_semantic_numeric_ranges() {
        let mut cfg = DevKitConfig::default();
        cfg.storage.max_entries = 0;
        let err = cfg.validate(Path::new(".")).unwrap_err();
        assert!(err.iter().any(|p| p.section == "storage" && p.key == "max_entries"));

        let mut cfg2 = DevKitConfig::default();
        cfg2.network.timeout_secs = Some(0);
        let err2 = cfg2.validate(Path::new(".")).unwrap_err();
        assert!(err2.iter().any(|p| p.section == "network" && p.key == "timeout_secs"));

        let mut cfg3 = DevKitConfig::default();
        cfg3.decode.max_depth = 0;
        assert!(cfg3.validate(Path::new(".")).is_ok());
    }

    #[test]
    fn test_semantic_contract_paths_and_cargo_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let no_cargo_dir = tmp.path().join("contracts/no_cargo");
        std::fs::create_dir_all(&no_cargo_dir).unwrap();

        let valid_dir = tmp.path().join("contracts/valid");
        std::fs::create_dir_all(&valid_dir).unwrap();
        std::fs::write(valid_dir.join("Cargo.toml"), "[package]\nname = \"v\"\n").unwrap();

        // Missing directory
        let cfg_missing = DevKitConfig::from_toml(
            "[contracts.missing]\npath = \"contracts/does_not_exist\"\n",
        )
        .unwrap();
        let err_missing = cfg_missing.validate(tmp.path()).unwrap_err();
        assert_eq!(err_missing[0].section, "contracts.missing");
        assert_eq!(err_missing[0].key, "path");

        // Directory exists without Cargo.toml
        let cfg_no_cargo = DevKitConfig::from_toml(
            "[contracts.nocargo]\npath = \"contracts/no_cargo\"\n",
        )
        .unwrap();
        let err_no_cargo = cfg_no_cargo.validate(tmp.path()).unwrap_err();
        assert_eq!(err_no_cargo[0].section, "contracts.nocargo");
        assert!(err_no_cargo[0].message.contains("Cargo.toml"));

        // Valid contract path
        let cfg_valid =
            DevKitConfig::from_toml("[contracts.valid]\npath = \"contracts/valid\"\n").unwrap();
        assert!(cfg_valid.validate(tmp.path()).is_ok());
    }

    #[test]
    fn test_semantic_dependencies_and_project_graph() {
        let tmp = tempfile::tempdir().unwrap();
        let c_a = tmp.path().join("contracts/a");
        let c_b = tmp.path().join("contracts/b");
        std::fs::create_dir_all(&c_a).unwrap();
        std::fs::create_dir_all(&c_b).unwrap();
        std::fs::write(c_a.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        std::fs::write(c_b.join("Cargo.toml"), "[package]\nname = \"b\"\n").unwrap();

        // path + git together -> invalid
        let mixed = DevKitConfig::from_toml(
            "[dependencies.foo]\npath = \"a\"\ngit = \"https://example.com/foo.git\"\ntag = \"v1\"\n",
        )
        .unwrap();
        assert!(mixed.validate(tmp.path()).is_err());

        // git with no ref and no version -> invalid
        let no_ref = DevKitConfig::from_toml(
            "[dependencies.foo]\ngit = \"https://example.com/foo.git\"\n",
        )
        .unwrap();
        assert!(no_ref.validate(tmp.path()).is_err());

        // git with no ref but valid version -> valid
        let git_ver = DevKitConfig::from_toml(
            "[dependencies.foo]\ngit = \"https://example.com/foo.git\"\nversion = \"^1.0\"\n",
        )
        .unwrap();
        assert!(git_ver.validate(tmp.path()).is_ok());

        // git with both tag and branch -> invalid
        let two_refs = DevKitConfig::from_toml(
            "[dependencies.foo]\ngit = \"https://example.com/foo.git\"\ntag = \"v1\"\nbranch = \"main\"\n",
        )
        .unwrap();
        assert!(two_refs.validate(tmp.path()).is_err());

        // cycle in contracts -> invalid
        let cycle = DevKitConfig::from_toml(
            "[contracts.a]\npath = \"contracts/a\"\ndepends_on = [\"b\"]\n[contracts.b]\npath = \"contracts/b\"\ndepends_on = [\"a\"]\n",
        )
        .unwrap();
        let err_cycle = cycle.validate(tmp.path()).unwrap_err();
        assert!(err_cycle.iter().any(|p| p.section == "contracts"));

        // unknown dependency -> invalid
        let unknown_dep = DevKitConfig::from_toml(
            "[contracts.a]\npath = \"contracts/a\"\ndepends_on = [\"ghost\"]\n",
        )
        .unwrap();
        assert!(unknown_dep.validate(tmp.path()).is_err());

        // self-dependency -> invalid
        let self_dep = DevKitConfig::from_toml(
            "[contracts.a]\npath = \"contracts/a\"\ndepends_on = [\"a\"]\n",
        )
        .unwrap();
        assert!(self_dep.validate(tmp.path()).is_err());

        // duplicate dependency across deploy_after and depends_on -> invalid
        let dup_dep = DevKitConfig::from_toml(
            "[contracts.a]\npath = \"contracts/a\"\ndeploy_after = [\"b\"]\ndepends_on = [\"b\"]\n[contracts.b]\npath = \"contracts/b\"\n",
        )
        .unwrap();
        assert!(dup_dep.validate(tmp.path()).is_err());
    }
}

