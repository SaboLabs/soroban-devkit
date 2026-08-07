//! Identity and Keystore Management.
//!
//! Stores `ed25519` keypairs securely using OS-portable config directories.
//! Keys are stored using strict permissions (`0600`) to prevent unauthorized access.

use crate::error::StorageError;
use directories::ProjectDirs;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use stellar_strkey::Strkey;

/// A local Soroban identity containing a named keypair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub public_key: String,
}

/// Keystore for managing named Soroban identities.
pub struct IdentityStore {
    dir: PathBuf,
}

impl IdentityStore {
    /// Initialize the keystore in the default OS-portable config directory (`~/.config/sdkt/identities/`).
    pub fn new() -> Result<Self, StorageError> {
        let proj_dirs = ProjectDirs::from("com", "SorobanDevKit", "sdkt").ok_or_else(|| {
            StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No valid home directory found",
            ))
        })?;

        Self::with_dir(proj_dirs.config_dir().join("identities"))
    }

    /// Initialize the keystore in a custom directory.
    pub fn with_dir<P: AsRef<Path>>(dir: P) -> Result<Self, StorageError> {
        let dir = dir.as_ref().to_path_buf();
        if !dir.exists() {
            fs::create_dir_all(&dir).map_err(StorageError::Io)?;
        }
        Ok(Self { dir })
    }

    /// Generate a new random ED25519 identity.
    pub fn generate(&self, name: &str) -> Result<Identity, StorageError> {
        self.ensure_name_valid(name)?;

        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);

        self.save_key(name, &signing_key)
    }

    /// Import an identity from a secret key (S...) string.
    pub fn import(&self, name: &str, secret_key: &str) -> Result<Identity, StorageError> {
        self.ensure_name_valid(name)?;

        let key = stellar_strkey::ed25519::PrivateKey::from_string(secret_key).map_err(|e| {
            StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid StrKey: {}", e),
            ))
        })?;

        let signing_key = SigningKey::from_bytes(&key.0);
        self.save_key(name, &signing_key)
    }

    /// Load an identity by name.
    pub fn get(&self, name: &str) -> Result<Identity, StorageError> {
        let path = self.dir.join(format!("{}.toml", name));
        if !path.exists() {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Identity '{}' not found", name),
            )));
        }

        let signing_key = self.load_key(&path)?;

        Ok(Identity {
            name: name.to_string(),
            public_key: Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(
                signing_key.verifying_key().to_bytes(),
            ))
            .to_string()
            .as_str()
            .to_string(),
        })
    }

    /// Delete an identity.
    pub fn remove(&self, name: &str) -> Result<(), StorageError> {
        let path = self.dir.join(format!("{}.toml", name));
        if path.exists() {
            fs::remove_file(path).map_err(StorageError::Io)?;
        }

        // If it was the default identity, clear the default symlink.
        let default_path = self.dir.join("default");
        if default_path.exists() {
            if let Ok(target) = fs::read_link(&default_path) {
                if target.file_stem().and_then(|s| s.to_str()) == Some(name) {
                    let _ = fs::remove_file(default_path);
                }
            }
        }
        Ok(())
    }

    /// List all stored identities.
    pub fn list(&self) -> Result<Vec<Identity>, StorageError> {
        let mut identities = Vec::new();

        for entry in fs::read_dir(&self.dir).map_err(StorageError::Io)? {
            let entry = entry.map_err(StorageError::Io)?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(identity) = self.get(name) {
                        identities.push(identity);
                    }
                }
            }
        }

        Ok(identities)
    }

    /// Set an identity as the default.
    pub fn set_default(&self, name: &str) -> Result<(), StorageError> {
        let target_path = self.dir.join(format!("{}.toml", name));
        if !target_path.exists() {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Identity '{}' not found", name),
            )));
        }

        let default_path = self.dir.join("default");
        if default_path.exists() {
            fs::remove_file(&default_path).map_err(StorageError::Io)?;
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(target_path, &default_path).map_err(StorageError::Io)?;

        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target_path, &default_path).map_err(StorageError::Io)?;

        #[cfg(not(any(unix, windows)))]
        std::fs::copy(target_path, &default_path).map_err(StorageError::Io)?;

        Ok(())
    }

    /// Load the default identity.
    pub fn get_default(&self) -> Result<Identity, StorageError> {
        let default_path = self.dir.join("default");
        if !default_path.exists() {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No default identity set",
            )));
        }

        let target_path = fs::read_link(&default_path).map_err(StorageError::Io)?;
        let name = target_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        self.get(name)
    }

    /// Load the signing key (`ed25519_dalek::SigningKey`) for a named identity.
    ///
    /// This is the keystore integration point used by transaction signing:
    /// the caller extracts the 32-byte seed via `signing_key.to_bytes()` and
    /// passes it to `sdkt_xdr::Ed25519Signer::from_seed`. The secret material
    /// is never exposed as a string; only the in-memory `SigningKey` is
    /// returned, and only for the duration the caller holds it.
    ///
    /// # Errors
    /// Returns [`StorageError`] if the identity does not exist or its secret
    /// cannot be loaded.
    pub fn load_signing_key(&self, name: &str) -> Result<SigningKey, StorageError> {
        let path = self.dir.join(format!("{}.toml", name));
        self.load_key(&path)
    }

    // --- Private Helpers ---

    fn ensure_name_valid(&self, name: &str) -> Result<(), StorageError> {
        if name.is_empty() || name == "default" || name.contains('/') || name.contains('\\') {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid identity name",
            )));
        }

        let path = self.dir.join(format!("{}.toml", name));
        if path.exists() {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Identity '{}' already exists", name),
            )));
        }
        Ok(())
    }

    fn save_key(&self, name: &str, signing_key: &SigningKey) -> Result<Identity, StorageError> {
        let path = self.dir.join(format!("{}.toml", name));

        let secret_str = stellar_strkey::Unredacted(&stellar_strkey::ed25519::PrivateKey(
            signing_key.to_bytes(),
        ))
        .to_string()
        .as_str()
        .to_string();

        let toml_content = format!(
            "# Soroban Identity: {}\nsecret_key = \"{}\"\n",
            name, secret_str
        );

        // Strict permissions 0600
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);

        #[cfg(unix)]
        opts.mode(0o600);

        let mut file = opts.open(&path).map_err(StorageError::Io)?;

        file.write_all(toml_content.as_bytes())
            .map_err(StorageError::Io)?;

        let pub_key_str = Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(
            signing_key.verifying_key().to_bytes(),
        ))
        .to_string()
        .as_str()
        .to_string();

        Ok(Identity {
            name: name.to_string(),
            public_key: pub_key_str,
        })
    }

    fn load_key(&self, path: &Path) -> Result<SigningKey, StorageError> {
        let content = fs::read_to_string(path).map_err(StorageError::Io)?;

        let mut secret = None;
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("secret_key") {
                let parts: Vec<&str> = line.split('=').collect();
                if parts.len() == 2 {
                    secret = Some(parts[1].trim().trim_matches('"').to_string());
                }
            }
        }

        let secret = secret.ok_or_else(|| {
            StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "secret_key not found in identity file",
            ))
        })?;

        let key = stellar_strkey::ed25519::PrivateKey::from_string(&secret).map_err(|e| {
            StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid stored StrKey: {}", e),
            ))
        })?;

        Ok(SigningKey::from_bytes(&key.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_generate_and_get() {
        let dir = tempdir().unwrap();
        let store = IdentityStore::with_dir(dir.path()).unwrap();

        let identity = store.generate("alice").unwrap();
        assert_eq!(identity.name, "alice");
        assert!(identity.public_key.starts_with('G'));

        let loaded = store.get("alice").unwrap();
        assert_eq!(loaded.name, "alice");
        assert_eq!(loaded.public_key, identity.public_key);
    }

    #[test]
    fn test_import_invalid_key() {
        let dir = tempdir().unwrap();
        let store = IdentityStore::with_dir(dir.path()).unwrap();

        assert!(store.import("bob", "not-a-key").is_err());
        assert!(store
            .import(
                "bob",
                "GBZXLHQZGOWBZY6W3U4Z7GZGGXYVQBZWYM3XEQZ7W5Z4QXYZ5Z3XYY"
            )
            .is_err()); // public key, not secret
    }

    #[test]
    fn test_list_and_remove() {
        let dir = tempdir().unwrap();
        let store = IdentityStore::with_dir(dir.path()).unwrap();

        store.generate("alice").unwrap();
        store.generate("bob").unwrap();

        let mut list = store.list().unwrap();
        list.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "alice");
        assert_eq!(list[1].name, "bob");

        store.remove("alice").unwrap();
        let list2 = store.list().unwrap();
        assert_eq!(list2.len(), 1);
        assert_eq!(list2[0].name, "bob");
    }

    #[test]
    fn test_default_identity() {
        let dir = tempdir().unwrap();
        let store = IdentityStore::with_dir(dir.path()).unwrap();

        let alice = store.generate("alice").unwrap();

        assert!(store.get_default().is_err());

        store.set_default("alice").unwrap();
        let def = store.get_default().unwrap();

        assert_eq!(def.name, "alice");
        assert_eq!(def.public_key, alice.public_key);
    }
}
