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
    /// Optional package manifest metadata (M35.0). Present when this project
    /// is itself a publishable package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageConfig>,
    /// Local path-only package dependencies (M35.0). Keys are package names;
    /// values are local path references. No git/HTTP/registry sources.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub dependencies: std::collections::HashMap<String, LocalDependency>,
}

/// Settings for a specific contract in a workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractConfig {
    /// Path to the contract source directory (where Cargo.toml resides).
    pub path: String,
    /// Optional list of contract aliases that must be deployed before this one.
    /// (`deploy_after` is the original spelling; `depends_on` is the canonical
    /// M34.2 field. Both are accepted and merged during resolution.)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deploy_after: Vec<String>,
    /// Canonical M34.2 dependency declaration. Semantically identical to
    /// `deploy_after`; listed here for explicit package-dependency graphs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

/// Package manifest metadata (M35.0).
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

/// A single local package dependency (M35.0).
///
/// Only path-based local dependencies are supported. `deny_unknown_fields`
/// rejects `git`, `version`, `registry`, or any remote-style key at parse time,
/// enforcing the "local path only, no network, no registry" constraint before
/// any validation logic runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct LocalDependency {
    /// Relative path (from the depending manifest) to the local package root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

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
    /// Limit max depth during nested XDR parsing.
    pub max_depth: usize,
    /// Fallback to hex decoding if base64 detection fails.
    pub allow_fallback_hex: bool,
}

/// Settings for Soroban storage inspection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageConfig {
    /// Maximum number of storage entries to fetch per page.
    pub max_entries: usize,
    /// Number of days before TTL expiration to start warning.
    pub ttl_warning_days: u32,
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
            max_depth: 32,
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

impl DevKitConfig {
    /// Loads configuration from a TOML string.
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
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
        assert_eq!(default_config.decode.max_depth, 32);
        assert!(default_config.decode.allow_fallback_hex);
        assert_eq!(default_config.storage.max_entries, 200);
        assert_eq!(default_config.storage.ttl_warning_days, 30);
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
}
