//! Persistent deployment records for `sdkt project deploy`.
//!
//! Every contract deployed by `sdkt project deploy` is recorded in a
//! `.sdkt-deployments.json` file next to `.sdkt.toml`, mapping each contract
//! alias to its on-chain contract ID. The record is written incrementally at
//! loop exit, so a deploy that fails halfway still preserves the contracts that
//! did land on-chain (see issue #74).
//!
//! Records are scoped **per network**: deployments to testnet and mainnet live
//! under different keys and never overwrite each other. A re-run with
//! `--skip-deployed` reads the record and skips any alias whose recorded
//! contract ID still exists on-chain (verified via `getLedgerEntries`, not just
//! by the presence of a file entry).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Default name of the deployment record file (written next to `.sdkt.toml`).
pub const DEPLOYMENT_RECORD_FILE: &str = ".sdkt-deployments.json";

/// Well-known passphrases mapped to human-readable network keys.
pub const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";
pub const MAINNET_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";
pub const FUTURENET_PASSPHRASE: &str = "Test SDF Future Network ; October 2022";

/// A single deployed-contract record for one alias on one network.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeploymentRecord {
    /// On-chain contract ID (StrKey `C...`).
    pub contract_id: String,
    /// SHA-256 WASM hash of the deployed artifact.
    pub wasm_hash: String,
    /// Network key this contract was deployed to (see [`network_key`]).
    pub network: String,
    /// Unix epoch seconds when the record was written.
    pub timestamp: u64,
    /// Deployment salt used to derive the contract ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
}

/// The on-disk record file: a map of network key → (alias → record).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DeploymentRecordFile {
    pub profiles: HashMap<String, HashMap<String, DeploymentRecord>>,
}

impl DeploymentRecordFile {
    /// Read the record file at `path`. A missing or empty file is treated as a
    /// fresh (empty) record set; a present-but-corrupt file is an error so the
    /// operator is told to inspect it rather than silently overwriting it.
    pub fn read<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(DeploymentRecordFile::default());
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(DeploymentRecordFile::default());
        }
        serde_json::from_str(&raw).map_err(|e| format!("Failed to parse {}: {e}", path.display()))
    }

    /// Write the record file to `path`. Uses an atomic write: serializes to a
    /// sibling temp file, syncs to disk, then renames over the target. This
    /// prevents a crash or full-disk mid-write from leaving a partial or empty
    /// file that would block subsequent deploys with a parse error.
    ///
    /// Serialization errors (none expected for this type) and I/O errors are
    /// returned to the caller, which warns but never fails the deployment on a
    /// record-write problem.
    pub fn write<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        use std::io::Write as _;

        let path = path.as_ref();
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize deployment record: {e}"))?;

        // Write to a temp file in the same directory so the rename is
        // guaranteed to be atomic on the same filesystem.
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let tmp_path = parent.join(format!(
            ".sdkt-deployments.tmp.{}",
            std::process::id()
        ));

        let mut tmp_file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temp record file {}: {e}", tmp_path.display()))?;
        tmp_file
            .write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temp record file {}: {e}", tmp_path.display()))?;
        tmp_file
            .sync_all()
            .map_err(|e| format!("Failed to sync temp record file {}: {e}", tmp_path.display()))?;
        drop(tmp_file);

        std::fs::rename(&tmp_path, path).map_err(|e| {
            // Best-effort cleanup; ignore secondary error.
            let _ = std::fs::remove_file(&tmp_path);
            format!("Failed to rename temp record file to {}: {e}", path.display())
        })
    }

    /// Records for a single network key, if any exist.
    pub fn records_for(&self, network: &str) -> Option<&HashMap<String, DeploymentRecord>> {
        self.profiles.get(network)
    }

    /// Record for a single alias on a network, if present.
    pub fn record_for(&self, network: &str, alias: &str) -> Option<&DeploymentRecord> {
        self.records_for(network)
            .and_then(|records| records.get(alias))
    }

    /// Insert (or replace) a record for an alias on a network.
    pub fn set_record(&mut self, network: &str, alias: &str, record: DeploymentRecord) {
        self.profiles
            .entry(network.to_string())
            .or_default()
            .insert(alias.to_string(), record);
    }

    /// Remove a record for an alias on a network (used when a recorded contract
    /// is found to no longer exist on-chain and gets re-deployed).
    pub fn remove_record(&mut self, network: &str, alias: &str) {
        if let Some(records) = self.profiles.get_mut(network) {
            records.remove(alias);
        }
    }
}

/// Derive the stable per-network record key from the resolved network settings.
///
/// When an explicit `--network-profile` was selected, the profile name is used
/// verbatim (a user can have distinct profiles for shared testnet RPC hosts).
/// Otherwise the key is derived from the network passphrase — the exact value
/// that determines the transaction signature hash — so testnet, mainnet,
/// futurenet, and any custom network never share a record scope. Custom
/// passphrases get a deterministic hash suffix so two different custom networks
/// cannot collide on a truncated slug.
pub fn network_key(network_profile: Option<&str>, passphrase: &str) -> String {
    if let Some(profile) = network_profile {
        if !profile.trim().is_empty() {
            return profile.to_string();
        }
    }

    match passphrase {
        TESTNET_PASSPHRASE => "testnet".to_string(),
        MAINNET_PASSPHRASE => "mainnet".to_string(),
        FUTURENET_PASSPHRASE => "futurenet".to_string(),
        other => {
            use sha2::Digest;
            let digest = sha2::Sha256::digest(other.as_bytes());
            let prefix: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
            format!("custom-{prefix}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(alias: &str, contract_id: &str, network: &str) -> DeploymentRecord {
        DeploymentRecord {
            contract_id: contract_id.to_string(),
            wasm_hash: "ab12cd34".to_string(),
            network: network.to_string(),
            timestamp: 1_700_000_000,
            salt: Some(alias.to_string()),
        }
    }

    #[test]
    fn record_file_write_read_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DEPLOYMENT_RECORD_FILE);

        let mut file = DeploymentRecordFile::default();
        file.set_record("testnet", "token", record("token", "CTOKEN", "testnet"));
        file.set_record("testnet", "vault", record("vault", "CVAULT", "testnet"));
        file.write(&path).unwrap();

        let loaded = DeploymentRecordFile::read(&path).unwrap();
        assert_eq!(loaded, file);
        assert_eq!(
            loaded.record_for("testnet", "token").unwrap().contract_id,
            "CTOKEN"
        );
        assert_eq!(
            loaded.record_for("testnet", "vault").unwrap().contract_id,
            "CVAULT"
        );
    }

    #[test]
    fn missing_record_file_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DEPLOYMENT_RECORD_FILE);
        let file = DeploymentRecordFile::read(&path).unwrap();
        assert!(file.profiles.is_empty());
    }

    #[test]
    fn corrupt_record_file_is_an_error_not_a_silent_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DEPLOYMENT_RECORD_FILE);
        std::fs::write(&path, "not json {").unwrap();
        assert!(DeploymentRecordFile::read(&path).is_err());
    }

    #[test]
    fn records_are_scoped_per_network_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DEPLOYMENT_RECORD_FILE);

        let mut file = DeploymentRecordFile::default();
        file.set_record("testnet", "token", record("token", "C_TESTNET", "testnet"));
        file.set_record("mainnet", "token", record("token", "C_MAINNET", "mainnet"));
        file.write(&path).unwrap();

        let loaded = DeploymentRecordFile::read(&path).unwrap();
        // Same alias, different networks -> different records that do not clash.
        assert_eq!(
            loaded.record_for("testnet", "token").unwrap().contract_id,
            "C_TESTNET"
        );
        assert_eq!(
            loaded.record_for("mainnet", "token").unwrap().contract_id,
            "C_MAINNET"
        );
        assert_eq!(loaded.records_for("testnet").unwrap().len(), 1);
        assert_eq!(loaded.records_for("mainnet").unwrap().len(), 1);
    }

    #[test]
    fn writing_testnet_then_mainnet_does_not_overwrite() {
        // Second write must merge, not replace, the other network's records.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DEPLOYMENT_RECORD_FILE);

        let mut testnet = DeploymentRecordFile::default();
        testnet.set_record("testnet", "token", record("token", "C_T", "testnet"));
        testnet.write(&path).unwrap();

        let mut mainnet = DeploymentRecordFile::read(&path).unwrap();
        mainnet.set_record("mainnet", "token", record("token", "C_M", "mainnet"));
        mainnet.write(&path).unwrap();

        let loaded = DeploymentRecordFile::read(&path).unwrap();
        assert_eq!(
            loaded.record_for("testnet", "token").unwrap().contract_id,
            "C_T"
        );
        assert_eq!(
            loaded.record_for("mainnet", "token").unwrap().contract_id,
            "C_M"
        );
    }

    #[test]
    fn record_lookup_and_replace_semantics() {
        let mut file = DeploymentRecordFile::default();
        assert!(file.record_for("testnet", "token").is_none());
        file.set_record("testnet", "token", record("token", "C1", "testnet"));
        assert_eq!(
            file.record_for("testnet", "token").unwrap().contract_id,
            "C1"
        );
        file.set_record("testnet", "token", record("token", "C2", "testnet"));
        assert_eq!(
            file.record_for("testnet", "token").unwrap().contract_id,
            "C2"
        );
        file.remove_record("testnet", "token");
        assert!(file.record_for("testnet", "token").is_none());
    }

    #[test]
    fn network_key_prefers_explicit_profile() {
        assert_eq!(
            network_key(Some("my-testnet"), TESTNET_PASSPHRASE),
            "my-testnet"
        );
        // Empty profile name falls through to the passphrase mapping.
        assert_eq!(network_key(Some("  "), TESTNET_PASSPHRASE), "testnet");
    }

    #[test]
    fn network_key_maps_known_passphrases() {
        assert_eq!(network_key(None, TESTNET_PASSPHRASE), "testnet");
        assert_eq!(network_key(None, MAINNET_PASSPHRASE), "mainnet");
        assert_eq!(network_key(None, FUTURENET_PASSPHRASE), "futurenet");
    }

    #[test]
    fn network_key_for_custom_passphrases_is_deterministic_and_distinct() {
        let a = network_key(None, "Standalone Network ; 2024");
        let b = network_key(None, "Standalone Network ; 2024");
        let c = network_key(None, "Custom Net / 2025");
        assert_eq!(a, b);
        assert_ne!(a, c);
        // Testnet and custom never collide even when slugs overlap.
        assert_ne!(network_key(None, TESTNET_PASSPHRASE), a);
    }
}
