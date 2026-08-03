//! Core configuration engine for the Soroban DevKit (sdkt).
//!
//! This module defines the global configuration structures and handles
//! the loading of configs from files, environment variables, and CLI overrides.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// High-level project-wide configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DevKitConfig {
    /// Soroban network settings.
    pub network: NetworkConfig,
    /// XDR decoder configurations.
    pub decode: DecodeConfig,
    /// Storage inspection settings.
    #[serde(default)]
    pub storage: StorageConfig,
}

/// Soroban network connection settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkConfig {
    /// Target Soroban RPC URL.
    pub rpc_url: String,
    /// Core passphrase matching target network.
    pub passphrase: String,
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

impl Default for DevKitConfig {
    fn default() -> Self {
        Self {
            network: NetworkConfig {
                rpc_url: "https://soroban-testnet.stellar.org".to_string(),
                passphrase: "Test SDF Network ; September 2015".to_string(),
            },
            decode: DecodeConfig {
                max_depth: 32,
                allow_fallback_hex: true,
            },
            storage: StorageConfig::default(),
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
}
