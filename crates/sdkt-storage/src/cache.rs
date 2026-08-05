//! File-system based WASM metadata cache.
//!
//! Stores parsed `WasmMetadata` and raw `.wasm` binaries locally to
//! avoid repeated RPC fetches for immutable contract code.

use crate::error::StorageError;
use directories::ProjectDirs;
use sdkt_wasm::WasmMetadata;
use std::fs;
use std::path::{Path, PathBuf};

/// Info about the current state of the WASM cache for a specific network.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheInfo {
    pub network: String,
    pub entry_count: usize,
    pub total_metadata_size_bytes: u64,
    pub total_wasm_size_bytes: u64,
}

/// A local file-system cache for Soroban WASM metadata and binaries.
pub struct WasmCache {
    base_dir: PathBuf,
}

impl WasmCache {
    /// Creates a new `WasmCache` instance.
    /// Uses the standard OS cache directory:
    /// - Linux: `~/.cache/soroban-devkit/`
    /// - macOS: `~/Library/Caches/org.naninu123.soroban-devkit/`
    /// - Windows: `%LOCALAPPDATA%\naninu123\soroban-devkit\cache\`
    pub fn new() -> Result<Self, StorageError> {
        let proj_dirs =
            ProjectDirs::from("org", "naninu123", "soroban-devkit").ok_or_else(|| {
                StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Could not determine OS cache directory",
                ))
            })?;

        let base_dir = proj_dirs.cache_dir().to_path_buf();
        Ok(Self { base_dir })
    }

    /// Creates a cache instance targeting a specific directory (useful for testing).
    pub fn with_dir<P: AsRef<Path>>(path: P) -> Self {
        Self {
            base_dir: path.as_ref().to_path_buf(),
        }
    }

    /// Returns the directory for a specific network (e.g., `testnet`).
    /// Creates the directory if it doesn't exist.
    fn network_dir(&self, network: &str) -> Result<PathBuf, StorageError> {
        let net_dir = self.base_dir.join("wasm").join(network);
        if !net_dir.exists() {
            fs::create_dir_all(&net_dir).map_err(StorageError::Io)?;
        }
        Ok(net_dir)
    }

    /// Returns `true` if metadata for the `wasm_hash` exists in the given `network`.
    pub fn contains(&self, network: &str, wasm_hash: &str) -> Result<bool, StorageError> {
        let net_dir = self.network_dir(network)?;
        let meta_path = net_dir.join(format!("{}.json", wasm_hash));
        Ok(meta_path.exists())
    }

    /// Retrieves parsed metadata from the cache.
    ///
    /// # Errors
    /// Returns `StorageError::CorruptCache` if the JSON is malformed.
    pub fn get(
        &self,
        network: &str,
        wasm_hash: &str,
    ) -> Result<Option<WasmMetadata>, StorageError> {
        let net_dir = self.network_dir(network)?;
        let meta_path = net_dir.join(format!("{}.json", wasm_hash));

        if !meta_path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&meta_path).map_err(StorageError::Io)?;
        let metadata: WasmMetadata = serde_json::from_str(&json).map_err(|e| {
            StorageError::CorruptCache(format!("Malformed JSON for {}: {}", wasm_hash, e))
        })?;

        Ok(Some(metadata))
    }

    /// Writes both metadata and raw WASM to the cache using atomic tempfile renaming.
    pub fn put(
        &self,
        network: &str,
        metadata: &WasmMetadata,
        wasm_bytes: &[u8],
    ) -> Result<(), StorageError> {
        let net_dir = self.network_dir(network)?;
        let hash = &metadata.hash;

        let meta_path = net_dir.join(format!("{}.json", hash));
        let wasm_path = net_dir.join(format!("{}.wasm", hash));

        let meta_temp = net_dir.join(format!("{}.json.tmp", hash));
        let wasm_temp = net_dir.join(format!("{}.wasm.tmp", hash));

        // Serialize JSON to string
        let json = serde_json::to_string(metadata)
            .map_err(|e| StorageError::Parse(format!("Failed to serialize metadata: {}", e)))?;

        // Write to temporary files first to prevent corruption
        fs::write(&meta_temp, json).map_err(StorageError::Io)?;
        fs::write(&wasm_temp, wasm_bytes).map_err(StorageError::Io)?;

        // Atomic rename
        fs::rename(&meta_temp, &meta_path).map_err(StorageError::Io)?;
        fs::rename(&wasm_temp, &wasm_path).map_err(StorageError::Io)?;

        Ok(())
    }

    /// Removes a specific WASM entry (both JSON and `.wasm`) from the cache.
    pub fn remove(&self, network: &str, wasm_hash: &str) -> Result<(), StorageError> {
        let net_dir = self.network_dir(network)?;
        let meta_path = net_dir.join(format!("{}.json", wasm_hash));
        let wasm_path = net_dir.join(format!("{}.wasm", wasm_hash));

        if meta_path.exists() {
            fs::remove_file(meta_path).map_err(StorageError::Io)?;
        }
        if wasm_path.exists() {
            fs::remove_file(wasm_path).map_err(StorageError::Io)?;
        }

        Ok(())
    }

    /// Clears all cached WASM entries for a specific network.
    pub fn clear(&self, network: &str) -> Result<(), StorageError> {
        let net_dir = self.network_dir(network)?;
        if net_dir.exists() {
            fs::remove_dir_all(&net_dir).map_err(StorageError::Io)?;
            fs::create_dir_all(&net_dir).map_err(StorageError::Io)?;
        }
        Ok(())
    }

    /// Retrieves usage statistics for a specific network.
    pub fn cache_info(&self, network: &str) -> Result<CacheInfo, StorageError> {
        let net_dir = self.network_dir(network)?;

        let mut entry_count = 0;
        let mut total_metadata_size_bytes = 0;
        let mut total_wasm_size_bytes = 0;

        if net_dir.exists() {
            for entry in fs::read_dir(net_dir).map_err(StorageError::Io)? {
                let entry = entry.map_err(StorageError::Io)?;
                let meta = entry.metadata().map_err(StorageError::Io)?;
                let name = entry.file_name().to_string_lossy().to_string();

                if name.ends_with(".json") {
                    entry_count += 1; // Count by JSON files
                    total_metadata_size_bytes += meta.len();
                } else if name.ends_with(".wasm") {
                    total_wasm_size_bytes += meta.len();
                }
            }
        }

        Ok(CacheInfo {
            network: network.to_string(),
            entry_count,
            total_metadata_size_bytes,
            total_wasm_size_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn get_temp_cache() -> (WasmCache, TempDir) {
        let tmp_dir = TempDir::new().unwrap();
        let cache = WasmCache::with_dir(tmp_dir.path());
        (cache, tmp_dir)
    }

    fn dummy_metadata(hash: &str) -> WasmMetadata {
        WasmMetadata {
            hash: hash.to_string(),
            size_bytes: 10,
            version: 1,
            exports: vec![],
            imports: vec![],
            custom_sections: vec![],
        }
    }

    #[test]
    fn test_put_get_contains() {
        let (cache, _dir) = get_temp_cache();
        let network = "testnet";
        let hash = "abcd123";

        assert!(!cache.contains(network, hash).unwrap());
        assert!(cache.get(network, hash).unwrap().is_none());

        let meta = dummy_metadata(hash);
        cache.put(network, &meta, b"1234567890").unwrap();

        assert!(cache.contains(network, hash).unwrap());
        let retrieved = cache.get(network, hash).unwrap().unwrap();
        assert_eq!(retrieved.hash, hash);
    }

    #[test]
    fn test_remove() {
        let (cache, _dir) = get_temp_cache();
        let meta = dummy_metadata("test_hash");

        cache.put("mainnet", &meta, b"xyz").unwrap();
        assert!(cache.contains("mainnet", "test_hash").unwrap());

        cache.remove("mainnet", "test_hash").unwrap();
        assert!(!cache.contains("mainnet", "test_hash").unwrap());

        // ensure files are physically gone
        let info = cache.cache_info("mainnet").unwrap();
        assert_eq!(info.entry_count, 0);
        assert_eq!(info.total_metadata_size_bytes, 0);
        assert_eq!(info.total_wasm_size_bytes, 0);
    }

    #[test]
    fn test_clear() {
        let (cache, _dir) = get_temp_cache();

        cache.put("testnet", &dummy_metadata("1"), b"").unwrap();
        cache.put("testnet", &dummy_metadata("2"), b"").unwrap();
        cache.put("mainnet", &dummy_metadata("3"), b"").unwrap();

        assert_eq!(cache.cache_info("testnet").unwrap().entry_count, 2);

        cache.clear("testnet").unwrap();

        assert_eq!(cache.cache_info("testnet").unwrap().entry_count, 0);
        assert_eq!(cache.cache_info("mainnet").unwrap().entry_count, 1);
    }

    #[test]
    fn test_corrupt_json() {
        let (cache, _dir) = get_temp_cache();
        let network = "testnet";
        let hash = "corrupt";

        // Create network dir and write garbage to the JSON file
        let net_dir = cache.network_dir(network).unwrap();
        fs::write(net_dir.join(format!("{}.json", hash)), b"{ invalid_json").unwrap();

        let err = cache.get(network, hash).unwrap_err();
        assert!(matches!(err, StorageError::CorruptCache(_)));
    }

    #[test]
    fn test_missing_wasm_is_fine_for_metadata_get() {
        let (cache, _dir) = get_temp_cache();
        let network = "testnet";
        let hash = "onlyjson";

        // Write only JSON
        let net_dir = cache.network_dir(network).unwrap();
        let json = serde_json::to_string(&dummy_metadata(hash)).unwrap();
        fs::write(net_dir.join(format!("{}.json", hash)), json).unwrap();

        let meta = cache.get(network, hash).unwrap().unwrap();
        assert_eq!(meta.hash, hash);
    }
}
