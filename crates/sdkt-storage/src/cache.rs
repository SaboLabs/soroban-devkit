//! File-system based WASM metadata cache.
//!
//! Stores parsed `WasmMetadata` and raw `.wasm` binaries locally to
//! avoid repeated RPC fetches for immutable contract code.

use crate::error::StorageError;
use directories::ProjectDirs;
use sdkt_wasm::WasmMetadata;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Info about the current state of the WASM cache for a specific network.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheInfo {
    pub network: String,
    pub entry_count: usize,
    pub total_metadata_size_bytes: u64,
    pub total_wasm_size_bytes: u64,
}

/// One enumerated entry of the WASM cache for a specific network.
///
/// `{hash}.json` and `{hash}.wasm` pair by filename stem; a missing half
/// reports `None` for its size (rendered "unknown" by the CLI) instead of
/// failing the enumeration.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheEntryInfo {
    pub network: String,
    /// Filename stem: the hash/key the entry is addressed by (the same
    /// value `remove` takes).
    pub hash: String,
    /// Size of the cached `{hash}.wasm` file in bytes, `None` when the
    /// binary half of the pair is absent.
    pub wasm_size_bytes: Option<u64>,
    /// Size of the cached `{hash}.json` metadata file in bytes, `None`
    /// when the metadata half of the pair is absent.
    pub metadata_size_bytes: Option<u64>,
    /// Diagnostic for this entry (e.g. malformed metadata). Never fatal:
    /// the listing still completes, matching how WASM metadata reads treat
    /// cache errors as warnings.
    pub warning: Option<String>,
}

/// A local file-system cache for Soroban WASM metadata and binaries.
pub struct WasmCache {
    base_dir: PathBuf,
}

impl WasmCache {
    /// Creates a new `WasmCache` instance.
    ///
    /// The directory location can be overridden via the `SDKT_CACHE_DIR` environment
    /// variable (checked first). If unset or empty, uses the standard OS cache directory:
    /// - Linux: `~/.cache/soroban-devkit/`
    /// - macOS: `~/Library/Caches/org.SaboLabs.soroban-devkit/`
    /// - Windows: `%LOCALAPPDATA%\SaboLabs\soroban-devkit\cache\`
    pub fn new() -> Result<Self, StorageError> {
        if let Ok(dir) = std::env::var("SDKT_CACHE_DIR") {
            let trimmed = dir.trim();
            if !trimmed.is_empty() {
                return Ok(Self {
                    base_dir: PathBuf::from(trimmed),
                });
            }
        }

        let proj_dirs =
            ProjectDirs::from("org", "SaboLabs", "soroban-devkit").ok_or_else(|| {
                StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Could not determine OS cache directory",
                ))
            })?;

        let base_dir = proj_dirs.cache_dir().to_path_buf();
        Ok(Self { base_dir })
    }

    /// Returns the base directory of the cache.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
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
        validate_network_name(network)?;
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
    ///
    /// A hash that is not cached is an error, not a silent success: callers
    /// print "Removed" on `Ok`, and reporting success for a no-op reads as a
    /// completed mutation to scripts and agents. Matches `identity delete` and
    /// `network remove`, which surface `Not found:` and exit non-zero.
    pub fn remove(&self, network: &str, wasm_hash: &str) -> Result<(), StorageError> {
        let net_dir = self.network_dir(network)?;
        let meta_path = net_dir.join(format!("{}.json", wasm_hash));
        let wasm_path = net_dir.join(format!("{}.wasm", wasm_hash));

        if !meta_path.exists() && !wasm_path.exists() {
            return Err(StorageError::NotFound(format!(
                "WASM cache entry '{}' not found in {} cache",
                wasm_hash, network
            )));
        }

        if meta_path.exists() {
            fs::remove_file(meta_path).map_err(StorageError::Io)?;
        }
        if wasm_path.exists() {
            fs::remove_file(wasm_path).map_err(StorageError::Io)?;
        }

        Ok(())
    }

    /// Clears all cached WASM entries for a specific network.
    ///
    /// Unlike `remove`, this stays truthful as a no-op success: "cleared" is
    /// accurate for an empty cache (the post-condition holds), and unlike a
    /// hash miss it is not evidence that a named target was deleted.
    pub fn clear(&self, network: &str) -> Result<(), StorageError> {
        let net_dir = self.network_dir(network)?;
        if net_dir.exists() {
            fs::remove_dir_all(&net_dir).map_err(StorageError::Io)?;
            fs::create_dir_all(&net_dir).map_err(StorageError::Io)?;
        }
        Ok(())
    }

    /// Retrieves usage statistics for a specific network.
    /// A missing or unreadable cache directory is treated as an empty cache
    /// (zero entries / zero size) on every platform. This keeps `cache info`
    /// working on fresh CI runners (Linux/macOS/Windows) where the cache path
    /// does not yet exist.
    pub fn cache_info(&self, network: &str) -> Result<CacheInfo, StorageError> {
        validate_network_name(network)?;
        let net_dir = self.base_dir.join("wasm").join(network);

        let mut entry_count = 0;
        let mut total_metadata_size_bytes = 0;
        let mut total_wasm_size_bytes = 0;

        // A missing or unreadable directory is an empty cache, not an error.
        // Swallowing the error (instead of `.map_err`) prevents ENOENT from
        // propagating on platforms where the path is absent or a symlink
        // target is missing (e.g. a fresh macOS runner).
        if let Ok(entries) = fs::read_dir(&net_dir) {
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
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

    /// Enumerates every cached entry for `network`, sorted by hash.
    ///
    /// Pairs `{hash}.json` with `{hash}.wasm` by filename stem and reports
    /// sizes from the filesystem; a half-pair reports `None` for the
    /// missing size instead of failing. Like [`Self::cache_info`], a
    /// missing or unreadable cache directory is an empty result, not an
    /// error (fresh CI runners). A malformed or unreadable `{hash}.json`
    /// produces a per-entry `warning` while the listing still completes.
    pub fn list_entries(&self, network: &str) -> Result<Vec<CacheEntryInfo>, StorageError> {
        validate_network_name(network)?;
        let net_dir = self.base_dir.join("wasm").join(network);

        // Collect pair halves keyed by filename stem. Temporary `.tmp`
        // files from interrupted writes match neither suffix and are
        // ignored, as are directories and unreadable directory entries.
        let mut pairs: BTreeMap<String, (Option<u64>, Option<u64>)> = BTreeMap::new();

        // A missing or unreadable directory is an empty cache, not an
        // error — same contract as `cache_info`.
        if let Ok(dir) = fs::read_dir(&net_dir) {
            for entry in dir {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let meta = match entry.metadata() {
                    Ok(m) if m.is_file() => m,
                    _ => continue,
                };
                let name = entry.file_name().to_string_lossy().to_string();
                let (stem, is_json) = if let Some(stem) = name.strip_suffix(".json") {
                    (stem, true)
                } else if let Some(stem) = name.strip_suffix(".wasm") {
                    (stem, false)
                } else {
                    continue;
                };
                if stem.is_empty() {
                    continue;
                }
                let slot = pairs.entry(stem.to_string()).or_insert((None, None));
                if is_json {
                    slot.0 = Some(meta.len());
                } else {
                    slot.1 = Some(meta.len());
                }
            }
        }

        // BTreeMap iteration is stem-sorted, so the ordering is
        // deterministic regardless of directory iteration order.
        let mut entries = Vec::with_capacity(pairs.len());
        for (hash, (metadata_size_bytes, wasm_size_bytes)) in pairs {
            // Validate this entry's metadata; a malformed file is a
            // warning on that entry only, never a listing failure.
            let warning = if metadata_size_bytes.is_some() {
                match fs::read_to_string(net_dir.join(format!("{}.json", hash))) {
                    Ok(json) => serde_json::from_str::<WasmMetadata>(&json)
                        .err()
                        .map(|e| format!("malformed metadata: {}", e)),
                    Err(e) => Some(format!("unreadable metadata: {}", e)),
                }
            } else {
                None
            };

            entries.push(CacheEntryInfo {
                network: network.to_string(),
                hash,
                wasm_size_bytes,
                metadata_size_bytes,
                warning,
            });
        }

        Ok(entries)
    }
}

fn validate_network_name(network: &str) -> Result<(), StorageError> {
    if network.is_empty()
        || network == "."
        || network == ".."
        || network.contains('/')
        || network.contains('\\')
        || !matches!(
            Path::new(network).components().next(),
            Some(Component::Normal(_))
        )
        || Path::new(network).components().nth(1).is_some()
    {
        return Err(StorageError::ConfigError(format!(
            "invalid network name '{}': must be a non-empty path component",
            network
        )));
    }
    Ok(())
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
            contract_meta: vec![],
            function_count: 0,
            memory: None,
            table_count: 0,
            global_count: 0,
            data_segment_count: 0,
            section_sizes: vec![],
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
    fn test_remove_missing_is_not_found_error() {
        // A no-op removal must not report success: callers print "Removed" on
        // Ok, and scripts/agents read that as a completed mutation.
        let (cache, _dir) = get_temp_cache();
        let err = cache
            .remove("mainnet", "never_cached")
            .expect_err("removing an absent entry must fail");
        assert!(
            matches!(&err, StorageError::NotFound(m) if m.contains("never_cached")),
            "expected NotFound, got {err:?}"
        );
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
    fn test_missing_cache_dir_is_empty() {
        // Simulate a fresh runner where the cache base dir does not exist yet
        // (the macOS CI failure scenario). cache_info must succeed with zero
        // counts instead of returning ENOENT.
        let tmp = TempDir::new().unwrap();
        // Point at a sub-path that does NOT exist.
        let cache = WasmCache::with_dir(tmp.path().join("does-not-exist-yet"));

        let info = cache.cache_info("testnet").unwrap();
        assert_eq!(info.entry_count, 0);
        assert_eq!(info.total_metadata_size_bytes, 0);
        assert_eq!(info.total_wasm_size_bytes, 0);
        assert_eq!(info.network, "testnet");
    }

    #[test]
    fn test_cache_info_empty_network_dir() {
        // Base dir exists but the per-network subdir does not.
        let (cache, _dir) = get_temp_cache();
        let info = cache.cache_info("nonexistent-network").unwrap();
        assert_eq!(info.entry_count, 0);
    }

    #[test]
    fn test_list_entries_lists_every_entry_with_sizes() {
        let (cache, _dir) = get_temp_cache();
        cache
            .put("testnet", &dummy_metadata("551c5a9c"), b"12345")
            .unwrap();
        cache
            .put("testnet", &dummy_metadata("661c5a9c"), b"")
            .unwrap();

        let entries = cache.list_entries("testnet").unwrap();
        assert_eq!(entries.len(), 2);
        let first = &entries[0];
        assert_eq!(first.hash, "551c5a9c");
        assert_eq!(first.wasm_size_bytes, Some(5));
        assert!(first.metadata_size_bytes.unwrap() > 0);
        assert!(first.warning.is_none());

        let second = &entries[1];
        assert_eq!(second.hash, "661c5a9c");
        assert_eq!(second.wasm_size_bytes, Some(0));
        assert!(second.warning.is_none());
    }

    #[test]
    fn test_list_entries_deterministic_order_and_network_isolation() {
        let (cache, _dir) = get_temp_cache();
        // Inserted out of lexicographic order across two networks.
        cache.put("testnet", &dummy_metadata("bbb"), b"").unwrap();
        cache.put("testnet", &dummy_metadata("aaa"), b"").unwrap();
        cache.put("mainnet", &dummy_metadata("zzz"), b"").unwrap();

        let entries = cache.list_entries("testnet").unwrap();
        let hashes: Vec<&str> = entries.iter().map(|e| e.hash.as_str()).collect();
        assert_eq!(hashes, ["aaa", "bbb"]);

        let mainnet = cache.list_entries("mainnet").unwrap();
        assert_eq!(mainnet.len(), 1);
        assert_eq!(mainnet[0].hash, "zzz");
    }

    #[test]
    fn test_list_entries_half_pair_reports_unknown_side() {
        let (cache, dir) = get_temp_cache();
        let net_dir = cache.network_dir("testnet").unwrap();

        // Binary only: no metadata, so no warning can be produced.
        std::fs::write(net_dir.join("onlywasm.wasm"), b"wasm-bytes").unwrap();
        // Metadata only: valid JSON, so the entry is clean.
        let meta_json = serde_json::to_string(&dummy_metadata("onlymeta")).unwrap();
        std::fs::write(net_dir.join("onlymeta.json"), meta_json).unwrap();

        let entries = cache.list_entries("testnet").unwrap();
        assert_eq!(entries.len(), 2);

        let only_wasm = entries.iter().find(|e| e.hash == "onlywasm").unwrap();
        assert_eq!(only_wasm.wasm_size_bytes, Some(10));
        assert_eq!(only_wasm.metadata_size_bytes, None);
        assert!(only_wasm.warning.is_none());

        let only_meta = entries.iter().find(|e| e.hash == "onlymeta").unwrap();
        assert_eq!(only_meta.wasm_size_bytes, None);
        assert!(only_meta.metadata_size_bytes.is_some());
        assert!(only_meta.warning.is_none());

        // The temp dir keeps the network dir alive for the closure-free reads.
        drop(dir);
    }

    #[test]
    fn test_list_entries_malformed_metadata_warns_but_listing_completes() {
        let (cache, dir) = get_temp_cache();
        let net_dir = cache.network_dir("testnet").unwrap();
        std::fs::write(net_dir.join("corrupt.json"), b"{ invalid_json").unwrap();
        std::fs::write(net_dir.join("corrupt.wasm"), b"abc").unwrap();

        let entries = cache.list_entries("testnet").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].hash, "corrupt");
        assert_eq!(entries[0].wasm_size_bytes, Some(3));
        let warning = entries[0].warning.as_deref().expect("warning expected");
        assert!(warning.contains("malformed metadata"), "got: {warning}");
        drop(dir);
    }

    #[test]
    fn test_list_entries_empty_and_missing_cache_dirs() {
        let (cache, _dir) = get_temp_cache();

        // Network subdir never created.
        let entries = cache.list_entries("nonexistent-network").unwrap();
        assert!(entries.is_empty());

        // Entire base dir absent (fresh runner).
        let tmp = TempDir::new().unwrap();
        let fresh = WasmCache::with_dir(tmp.path().join("does-not-exist-yet"));
        assert!(fresh.list_entries("testnet").unwrap().is_empty());
    }

    #[test]
    fn test_list_entries_ignores_temp_files_and_directories() {
        let (cache, _dir) = get_temp_cache();
        let net_dir = cache.network_dir("testnet").unwrap();
        // Leftovers from an interrupted atomic put.
        std::fs::write(net_dir.join("abc123.json.tmp"), b"{}").unwrap();
        std::fs::write(net_dir.join("abc123.wasm.tmp"), b"abc").unwrap();
        // A stray subdirectory must not become an entry.
        std::fs::create_dir_all(net_dir.join("subdir.json")).unwrap();

        let entries = cache.list_entries("testnet").unwrap();
        assert!(entries.is_empty(), "got: {entries:?}");
    }

    #[test]
    fn rejects_path_traversal_before_filesystem_access() {
        let (cache, dir) = get_temp_cache();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();

        assert!(matches!(
            cache.clear("../outside"),
            Err(StorageError::ConfigError(_))
        ));
        assert!(outside.exists());
        assert!(matches!(
            cache.remove("nested/name", "hash"),
            Err(StorageError::ConfigError(_))
        ));
        assert!(matches!(
            cache.cache_info(""),
            Err(StorageError::ConfigError(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_drive_relative_network_name() {
        let (cache, _dir) = get_temp_cache();

        assert!(matches!(
            cache.clear("C:outside"),
            Err(StorageError::ConfigError(_))
        ));
    }

    // ---------------------------------------------------------------------------
    // Windows-specific regression tests
    // ---------------------------------------------------------------------------

    /// Verifies the production WasmCache::new() constructor resolves a
    /// platform-appropriate cache directory on Windows.
    ///
    /// The test is read-only against the production constructor: it does not
    /// write any cache data. It only verifies the resolved base directory
    /// contains the expected OS-managed namespace.
    #[cfg(windows)]
    #[test]
    fn test_new_resolves_windows_cache_path() {
        let cache = WasmCache::new().expect("WasmCache::new() must succeed on Windows");

        // Windows: %LOCALAPPDATA%\SaboLabs\soroban-devkit\cache\
        // We do not assert an absolute path (OS-managed); only the namespace.
        let path_str = cache.base_dir.to_string_lossy().to_lowercase();
        assert!(
            path_str.contains("sabolabs"),
            "Windows cache path should contain the 'SaboLabs' namespace, got: {}",
            cache.base_dir.display()
        );
        assert!(
            path_str.contains("soroban-devkit"),
            "Windows cache path should contain the 'soroban-devkit' app name, got: {}",
            cache.base_dir.display()
        );
    }

    #[test]
    fn test_new_with_env_var_override() {
        let tmp = TempDir::new().unwrap();
        let key = "SDKT_CACHE_DIR";
        let prev = std::env::var_os(key);
        std::env::set_var(key, tmp.path());

        let cache = WasmCache::new().expect("WasmCache::new() with SDKT_CACHE_DIR");
        assert_eq!(cache.base_dir(), tmp.path());

        if let Some(p) = prev {
            std::env::set_var(key, p);
        } else {
            std::env::remove_var(key);
        }
    }
}
