//! Network profile storage for the Soroban DevKit.
//!
//! Manages named network configurations that can be referenced by profile name
//! in CLI commands instead of specifying full RPC URLs and network passphrases.

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::StorageError;

/// A named network profile with RPC endpoint and network passphrase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkProfile {
    /// Human-readable name for this profile
    pub name: String,
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Network passphrase (e.g., "Test SDF Network ; September 2015")
    pub network_passphrase: String,
    /// Optional: default friendbot URL for test networks
    pub friendbot_url: Option<String>,
    /// Optional: description or notes
    pub description: Option<String>,
}

impl NetworkProfile {
    /// Create a new network profile.
    pub fn new(
        name: impl Into<String>,
        rpc_url: impl Into<String>,
        network_passphrase: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            rpc_url: rpc_url.into(),
            network_passphrase: network_passphrase.into(),
            friendbot_url: None,
            description: None,
        }
    }

    /// Set the friendbot URL.
    pub fn with_friendbot(mut self, url: impl Into<String>) -> Self {
        self.friendbot_url = Some(url.into());
        self
    }

    /// Set the description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Validate the profile's required fields.
    ///
    /// Returns [`StorageError::ConfigError`] if the name or RPC URL is empty,
    /// or if the name contains a path separator (which would break the
    /// on-disk filename). The network passphrase is allowed to be empty only
    /// for local dev networks, but a missing RPC URL is always invalid.
    pub fn validate(&self) -> Result<(), StorageError> {
        if self.name.is_empty() {
            return Err(StorageError::ConfigError(
                "Network profile name is empty".into(),
            ));
        }
        if self.name.contains('/') || self.name.contains('\\') {
            return Err(StorageError::ConfigError(format!(
                "Network profile name '{}' must not contain a path separator",
                self.name
            )));
        }
        if self.rpc_url.is_empty() {
            return Err(StorageError::ConfigError(format!(
                "Network profile '{}' has an empty RPC URL",
                self.name
            )));
        }
        Ok(())
    }
}

/// Storage for network profiles.
///
/// Profiles are stored as individual JSON files in a directory, similar to identities.
/// The directory location can be overridden via `SDKT_NETWORK_DIR` environment variable
/// for testing or custom configurations; otherwise it uses the OS config directory
/// (`~/.config/sdkt/networks` on Linux, matching the `IdentityStore` convention).
pub struct NetworkStore {
    base_dir: PathBuf,
}

impl NetworkStore {
    /// Create a new network store.
    ///
    /// If the `SDKT_NETWORK_DIR` environment variable is set to a non-empty value,
    /// it is used as the base directory. This is the canonical cross-platform way for
    /// tests and CI to isolate the network store (mirrors `SDKT_IDENTITY_DIR`).
    ///
    /// Otherwise, falls back to the OS config directory via `directories::ProjectDirs`:
    /// - Linux:   `$XDG_CONFIG_HOME/sdkt/networks` or `~/.config/sdkt/networks`
    /// - macOS:   `~/Library/Application Support/sdkt/networks`
    /// - Windows: `%APPDATA%\sdkt\networks`
    pub fn new() -> Result<Self, StorageError> {
        if let Ok(dir) = std::env::var("SDKT_NETWORK_DIR") {
            if !dir.is_empty() {
                return Self::with_dir(dir);
            }
        }

        let proj_dirs = ProjectDirs::from("com", "SorobanDevKit", "sdkt").ok_or_else(|| {
            StorageError::ConfigError("Cannot determine config directory: no home dir".into())
        })?;

        Self::with_dir(proj_dirs.config_dir().join("networks"))
    }

    /// Create a network store rooted at an explicit directory.
    ///
    /// The directory is created if it does not already exist. This is the
    /// injection point used by tests and by callers that want to manage
    /// profiles in a non-default location without touching environment state.
    pub fn with_dir<P: AsRef<Path>>(dir: P) -> Result<Self, StorageError> {
        let base_dir = dir.as_ref().to_path_buf();

        fs::create_dir_all(&base_dir).map_err(StorageError::Io)?;

        Ok(Self { base_dir })
    }

    /// Get the path for a specific profile.
    fn profile_path(&self, name: &str) -> PathBuf {
        self.base_dir.join(format!("{}.json", name))
    }

    /// List all network profiles.
    pub fn list(&self) -> Result<Vec<NetworkProfile>, StorageError> {
        let mut profiles = Vec::new();

        let entries = fs::read_dir(&self.base_dir).map_err(StorageError::Io)?;

        for entry in entries {
            let entry = entry.map_err(StorageError::Io)?;

            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                match self.load_profile_from_path(&path) {
                    Ok(profile) => profiles.push(profile),
                    Err(_) => {
                        // Skip invalid profile files
                        continue;
                    }
                }
            }
        }

        // Sort by name for consistent output
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(profiles)
    }

    /// Get a specific network profile by name.
    pub fn get(&self, name: &str) -> Result<NetworkProfile, StorageError> {
        let path = self.profile_path(name);
        if !path.exists() {
            return Err(StorageError::NotFound(format!(
                "Network profile '{}' not found",
                name
            )));
        }
        self.load_profile_from_path(&path)
    }

    /// Load a profile from a specific path.
    fn load_profile_from_path(&self, path: &Path) -> Result<NetworkProfile, StorageError> {
        let content = fs::read_to_string(path).map_err(StorageError::Io)?;

        let profile: NetworkProfile = serde_json::from_str(&content).map_err(|e| {
            StorageError::Parse(format!("Failed to parse {}: {}", path.display(), e))
        })?;

        Ok(profile)
    }

    /// Add or update a network profile.
    pub fn add(&self, profile: NetworkProfile) -> Result<(), StorageError> {
        profile.validate()?;

        let path = self.profile_path(&profile.name);
        let content = serde_json::to_string_pretty(&profile)
            .map_err(|e| StorageError::Parse(format!("Failed to serialize profile: {}", e)))?;

        fs::write(&path, content).map_err(StorageError::Io)?;

        Ok(())
    }

    /// Remove a network profile.
    pub fn remove(&self, name: &str) -> Result<(), StorageError> {
        let path = self.profile_path(name);
        if !path.exists() {
            return Err(StorageError::NotFound(format!(
                "Network profile '{}' not found",
                name
            )));
        }

        fs::remove_file(&path).map_err(StorageError::Io)?;

        Ok(())
    }

    /// Check if a profile exists.
    pub fn exists(&self, name: &str) -> bool {
        self.profile_path(name).exists()
    }

    /// Get all profiles as a map keyed by profile name.
    pub fn as_map(&self) -> Result<HashMap<String, NetworkProfile>, StorageError> {
        let profiles = self.list()?;
        let mut map = HashMap::new();
        for profile in profiles {
            map.insert(profile.name.clone(), profile);
        }
        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_network_store_with_dir_add_get_remove() {
        let temp_dir = tempdir().unwrap();
        let store = NetworkStore::with_dir(temp_dir.path()).unwrap();

        // Add a profile
        let profile = NetworkProfile::new(
            "testnet",
            "https://soroban-testnet.stellar.org",
            "Test SDF Network ; September 2015",
        )
        .with_friendbot("https://friendbot.stellar.org")
        .with_description("Stellar testnet");

        store.add(profile.clone()).unwrap();

        // List profiles
        let profiles = store.list().unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "testnet");

        // Get profile
        let retrieved = store.get("testnet").unwrap();
        assert_eq!(retrieved.rpc_url, profile.rpc_url);
        assert_eq!(retrieved.network_passphrase, profile.network_passphrase);
        assert_eq!(
            retrieved.friendbot_url.as_deref(),
            Some("https://friendbot.stellar.org")
        );
        assert_eq!(retrieved.description.as_deref(), Some("Stellar testnet"));

        // Exists + remove
        assert!(store.exists("testnet"));
        store.remove("testnet").unwrap();
        assert!(!store.exists("testnet"));
    }

    #[test]
    fn test_get_missing_profile_errors() {
        let temp_dir = tempdir().unwrap();
        let store = NetworkStore::with_dir(temp_dir.path()).unwrap();

        let result = store.get("nonexistent");
        assert!(matches!(result, Err(StorageError::NotFound(_))));
        assert!(!store.exists("nonexistent"));
    }

    #[test]
    fn test_add_overwrites_existing_profile() {
        let temp_dir = tempdir().unwrap();
        let store = NetworkStore::with_dir(temp_dir.path()).unwrap();

        let first = NetworkProfile::new("net", "https://old.example", "Old Passphrase");
        store.add(first).unwrap();

        let updated = NetworkProfile::new("net", "https://new.example", "New Passphrase")
            .with_description("updated");
        store.add(updated).unwrap();

        // Still exactly one profile after overwrite.
        let profiles = store.list().unwrap();
        assert_eq!(profiles.len(), 1);

        let retrieved = store.get("net").unwrap();
        assert_eq!(retrieved.rpc_url, "https://new.example");
        assert_eq!(retrieved.network_passphrase, "New Passphrase");
        assert_eq!(retrieved.description.as_deref(), Some("updated"));
    }

    #[test]
    fn test_as_map_keys_by_name() {
        let temp_dir = tempdir().unwrap();
        let store = NetworkStore::with_dir(temp_dir.path()).unwrap();

        store
            .add(NetworkProfile::new("alpha", "https://a.example", "A"))
            .unwrap();
        store
            .add(NetworkProfile::new("beta", "https://b.example", "B"))
            .unwrap();

        let map = store.as_map().unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("alpha").unwrap().rpc_url, "https://a.example");
        assert_eq!(map.get("beta").unwrap().rpc_url, "https://b.example");
    }

    #[test]
    fn test_validate_rejects_empty_name_and_rpc() {
        let empty_name = NetworkProfile::new("", "https://x.example", "P");
        assert!(empty_name.validate().is_err());

        let sep_name = NetworkProfile::new("a/b", "https://x.example", "P");
        assert!(sep_name.validate().is_err());

        let empty_rpc = NetworkProfile::new("ok", "", "P");
        assert!(empty_rpc.validate().is_err());

        let valid = NetworkProfile::new("ok", "https://x.example", "P");
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn test_invalid_profile_json_is_skipped_on_list() {
        let temp_dir = tempdir().unwrap();
        let store = NetworkStore::with_dir(temp_dir.path()).unwrap();

        // Write a valid profile.
        store
            .add(NetworkProfile::new("good", "https://good.example", "G"))
            .unwrap();

        // Write a corrupt profile file directly into the store directory.
        let bad_path = store.profile_path("corrupt");
        fs::write(&bad_path, "{ not valid json").unwrap();

        let profiles = store.list().unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "good");
    }

    #[test]
    fn test_remove_missing_profile_errors() {
        let temp_dir = tempdir().unwrap();
        let store = NetworkStore::with_dir(temp_dir.path()).unwrap();

        let result = store.remove("ghost");
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }
}
