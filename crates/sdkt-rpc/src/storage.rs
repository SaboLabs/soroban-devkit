use crate::client::SorobanRpcClient;
use crate::error::RpcError;
use serde::Serialize;

/// Storage TTL info for a contract.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TtlInfo {
    pub contract_id: String,
    pub entries: Vec<TtlEntry>,
}

/// A single storage entry with human-readable TTL information.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TtlEntry {
    pub key: String,
    pub current_ttl: u32,
    pub expiration_time: String,
    pub days_remaining: u32,
    pub extension_cost_stroops: u64,
}

/// Calculates extension cost using a simple placeholder formula
/// because the exact design logic is unspecified.
pub fn calculate_extension_cost(ledger_delta: u32) -> u64 {
    ledger_delta as u64 * 100
}

/// Build the ledger key for a contract's **instance singleton** — the
/// `LedgerKey::ContractData` entry whose `key` is `ScVal::LedgerKeyContractInstance`.
///
/// This key is always present for a deployed contract, so it is the minimal valid
/// key to pass to `getLedgerEntries` when no explicit storage keys are known. Soroban
/// RPC requires explicit keys and cannot enumerate a contract's full storage, so the
/// instance entry is the guaranteed baseline (carrying the contract's WASM hash).
///
/// `contract_id` may be a StrKey `C...` or a raw 32-byte hex string; both are
/// normalized via [`crate::inspect::contract_id_to_hex`].
pub(crate) fn instance_ledger_key(contract_id: &str) -> Result<String, RpcError> {
    let contract_id_hex = crate::inspect::contract_id_to_hex(contract_id)?;
    sdkt_xdr::encode_ledger_key(&sdkt_xdr::LedgerKeyParams::ContractData(contract_id_hex))
        .map_err(|e| RpcError::Rpc(format!("Failed to encode instance ledger key: {e}")))
}

/// Fetches TTL info from the RPC node and transforms it into a `TtlInfo` representation.
pub async fn get_ttl_info(
    client: &SorobanRpcClient,
    contract_id: &str,
) -> Result<TtlInfo, RpcError> {
    let ledger_info = client.get_ledger().await?;
    let current_ledger = ledger_info.sequence;

    // Soroban RPC `getLedgerEntries` requires explicit keys — it cannot enumerate all
    // storage for a contract. The one key that is ALWAYS present for a deployed
    // contract is its instance singleton (see [`instance_ledger_key`]). Querying it
    // returns the contract's instance entry (real, decodable) and avoids the
    // "no keys specified in request" error that an empty key set causes. Further
    // storage data entries (persistent/temporary) would require explicit keys the
    // caller must supply; the instance entry is the guaranteed baseline.
    let instance_key = instance_ledger_key(contract_id)?;

    let storage_resp = client
        .get_contract_storage(contract_id, &[instance_key])
        .await?;

    let mut entries = Vec::new();
    for entry in storage_resp.entries {
        let current_ttl = if let Some(live_until) = entry.live_until_ledger_seq {
            live_until.saturating_sub(current_ledger)
        } else {
            0
        };

        // Rough estimation: 1 ledger ≈ 5 seconds
        let days_remaining = (current_ttl * 5) / (24 * 3600);
        let extension_cost_stroops = calculate_extension_cost(current_ttl);

        entries.push(TtlEntry {
            key: entry.key,
            current_ttl,
            expiration_time: format!("~{} days", days_remaining),
            days_remaining,
            extension_cost_stroops,
        });
    }

    Ok(TtlInfo {
        contract_id: contract_id.to_string(),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use stellar_xdr::{LedgerKey, ReadXdr};

    #[test]
    fn test_calculate_extension_cost() {
        const SINGLE_ENTRY_TTL_30: u32 = 30;
        const SINGLE_ENTRY_TTL_100: u32 = 100;

        assert_eq!(calculate_extension_cost(SINGLE_ENTRY_TTL_30), 3000);
        assert_eq!(calculate_extension_cost(SINGLE_ENTRY_TTL_100), 10000);
        assert_eq!(calculate_extension_cost(0), 0);
    }

    /// Proves the instance-ledger-key derivation never produces an empty/placeholder
    /// key: for a real contract id it returns a non-empty base64 XDR `LedgerKey`
    /// encoding the contract's instance singleton, and the StrKey and hex forms of the
    /// same contract yield an identical key.
    #[test]
    fn test_instance_ledger_key_is_valid_and_stable() {
        let c = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
        let hex = "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c";

        let key_from_strkey = instance_ledger_key(c).expect("StrKey C... should derive a key");
        let key_from_hex = instance_ledger_key(hex).expect("hex should derive a key");

        // Never empty / never a placeholder.
        assert!(!key_from_strkey.is_empty());
        assert_eq!(
            key_from_strkey, key_from_hex,
            "StrKey and hex must map to the same contract instance key"
        );

        // Decodes to a ContractData ledger key (the instance singleton).
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(key_from_strkey.trim())
            .expect("key must be valid base64");
        let mut cursor = std::io::Cursor::new(bytes);
        let mut limited = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let ledger_key = LedgerKey::read_xdr(&mut limited).expect("key must decode to a LedgerKey");
        match ledger_key {
            LedgerKey::ContractData(cd) => {
                assert!(
                    matches!(cd.key, stellar_xdr::ScVal::LedgerKeyContractInstance),
                    "instance key must target the contract instance singleton"
                );
            }
            other => panic!("expected LedgerKey::ContractData, got {other:?}"),
        }
    }

    #[test]
    fn test_instance_ledger_key_rejects_garbage() {
        // Garbage must error rather than silently yielding an empty/placeholder key.
        assert!(instance_ledger_key("not-a-contract-id").is_err());
    }
}
