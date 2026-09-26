use crate::client::SorobanRpcClient;
use crate::error::RpcError;
use base64::Engine;
use serde::Serialize;
use stellar_xdr::{LedgerEntryData, ReadXdr, SorobanTransactionData};

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

/// Result of a successful `ExtendFootprintTtl` submission.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtendResult {
    pub contract_id: String,
    pub extend_to: u32,
    pub footprint_keys: Vec<String>,
    pub hash: String,
    pub status: String,
    pub fee: u32,
}

/// Parse `min_resource_fee` from simulation. Never silently default to zero.
fn parse_min_resource_fee(raw: &str) -> Result<u32, RpcError> {
    let parsed: u64 = raw.parse().map_err(|_| {
        RpcError::Rpc(format!(
            "simulation returned invalid min_resource_fee: {raw:?}"
        ))
    })?;
    u32::try_from(parsed)
        .map_err(|_| RpcError::Rpc(format!("simulation min_resource_fee overflowed u32: {raw}")))
}

/// Merge the contract instance key with extra operator-supplied keys.
/// Extra keys may be base64 XDR or even-length hex XDR. Duplicates are dropped.
pub fn collect_extend_keys(
    contract_id: &str,
    extra_keys: &[String],
) -> Result<Vec<String>, RpcError> {
    let mut keys = vec![instance_ledger_key(contract_id)?];
    keys.extend(extra_keys.iter().cloned());
    sdkt_xdr::merge_footprint_keys(&keys)
        .map_err(|e| RpcError::Rpc(format!("invalid LedgerKey: {e}")))
}

/// Extend the TTL of known footprint keys via `ExtendFootprintTtl`.
///
/// Always includes the contract instance singleton. Additional keys may be
/// supplied; they are not discovered automatically.
///
/// `extend_to` is relative, as the protocol defines it: the entries will live
/// at least `extend_to` ledgers past the last closed ledger. It is passed to
/// the operation unchanged; adding the current ledger would overshoot and
/// exceed the network's maximum TTL.
pub async fn extend_footprint(
    client: &SorobanRpcClient,
    contract_id: &str,
    extra_keys: &[String],
    extend_to: u32,
    source_account: &str,
    signer: &sdkt_xdr::sign::Ed25519Signer,
    network: sdkt_xdr::sign::Network,
) -> Result<ExtendResult, RpcError> {
    if extend_to == 0 {
        return Err(RpcError::Rpc(
            "--ledgers / extend_to must be greater than 0".into(),
        ));
    }

    let footprint_keys = collect_extend_keys(contract_id, extra_keys)?;
    let sequence = crate::account::get_next_sequence(client, source_account).await?;

    let sim_params = sdkt_xdr::ExtendFootprintParams {
        source_account: source_account.to_string(),
        sequence,
        fee: 100,
        extend_to,
        footprint_keys: footprint_keys.clone(),
    };

    let initial_envelope = sdkt_xdr::build_extend_footprint_tx(&sim_params)
        .map_err(|e| RpcError::Rpc(format!("Failed to build extend transaction: {e}")))?;

    let simulation = crate::simulate::simulate_transaction(client, &initial_envelope)
        .await
        .map_err(|e| RpcError::Rpc(format!("Extend simulation failed: {e}")))?;

    if let Some(err) = &simulation.error {
        return Err(RpcError::Rpc(format!("Extend simulation error: {err}")));
    }
    if simulation.transaction_data.is_empty() {
        return Err(RpcError::Rpc(
            "Simulation did not return SorobanTransactionData".into(),
        ));
    }

    let soroban_data: SorobanTransactionData =
        sdkt_xdr::parse_soroban_transaction_data(&simulation.transaction_data)
            .map_err(|e| RpcError::Rpc(format!("Failed to parse SorobanTransactionData: {e}")))?;

    let min_resource_fee = parse_min_resource_fee(&simulation.min_resource_fee)?;
    let inclusion_fee: u32 = 100;
    let total_fee = inclusion_fee.saturating_add(min_resource_fee);

    let final_params = sdkt_xdr::ExtendFootprintParams {
        fee: total_fee,
        ..sim_params
    };
    let final_envelope = sdkt_xdr::build_extend_footprint_tx_with_data(&final_params, soroban_data)
        .map_err(|e| RpcError::Rpc(format!("Failed to build final extend transaction: {e}")))?;

    let signing_opts = sdkt_xdr::sign::SigningOptions::with(network);
    let signed_envelope = sdkt_xdr::sign_transaction(&final_envelope, signer, &signing_opts)
        .map_err(|e| RpcError::Rpc(format!("Failed to sign extend transaction: {e}")))?;

    let submission = crate::submission::submit_and_wait(
        client,
        &signed_envelope,
        true,
        &crate::submission::PollConfig::default(),
    )
    .await?;

    if submission.status != crate::submission::TransactionStatus::Success {
        let code = submission.error_code.as_deref().unwrap_or("unknown");
        let diag = submission.error_result_xdr.as_deref().unwrap_or("");
        return Err(RpcError::Rpc(format!(
            "Extend transaction failed: code={code} {diag}"
        )));
    }

    Ok(ExtendResult {
        contract_id: contract_id.to_string(),
        extend_to,
        footprint_keys,
        hash: submission.hash,
        status: "SUCCESS".into(),
        fee: total_fee,
    })
}

/// Read a single ledger entry by its `LedgerKey` (base64 XDR), together with
/// the RPC-reported `liveUntilLedgerSeq` for that entry.
///
/// Calls `getLedgerEntries` and returns the decoded `LedgerEntry` plus its
/// absolute expiry ledger (if the network reports one). If the requested entry
/// is absent, returns a clear error rather than an empty value.
pub async fn read_ledger_entry_with_ttl(
    client: &SorobanRpcClient,
    key_b64: &str,
) -> Result<(stellar_xdr::LedgerEntry, Option<u32>), RpcError> {
    let keys = vec![key_b64.to_string()];
    let response = client.get_contract_storage("", &keys).await?;

    if response.entries.is_empty() {
        return Err(RpcError::Rpc(
            "Ledger entry not found for the requested key".into(),
        ));
    }

    let result = &response.entries[0];
    let entry_bytes = base64::engine::general_purpose::STANDARD
        .decode(result.xdr.trim())
        .map_err(|e| RpcError::Rpc(format!("Failed to decode LedgerEntry XDR: {e}")))?;

    let mut cursor = std::io::Cursor::new(&entry_bytes);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    let entry = stellar_xdr::LedgerEntry::read_xdr(&mut l)
        .map_err(|e| RpcError::Rpc(format!("Failed to parse LedgerEntry: {e}")))?;

    Ok((entry, result.live_until_ledger_seq))
}

/// Read a single ledger entry by its `LedgerKey` (base64 XDR).
///
/// Calls `getLedgerEntries` and returns the decoded `LedgerEntry`. If the
/// requested entry is absent, returns a clear error rather than an empty value.
pub async fn read_ledger_entry(
    client: &SorobanRpcClient,
    key_b64: &str,
) -> Result<stellar_xdr::LedgerEntry, RpcError> {
    read_ledger_entry_with_ttl(client, key_b64)
        .await
        .map(|(entry, _live_until)| entry)
}

/// Result of a successful `storage read` call.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StateReadResult {
    pub contract_id: String,
    pub key: String,
    pub entry_type: String,
    pub durability: Option<String>,
    pub value: serde_json::Value,
    pub live_until_ledger: Option<u32>,
    /// Ledgers left before the entry expires, relative to the network's current
    /// ledger. `None` when the entry reports no expiry, or when the caller did
    /// not resolve the current ledger. Set by `sdkt storage read`.
    pub ledgers_remaining: Option<u32>,
    /// Whether [`Self::ledgers_remaining`] is at or below the toolkit's shared
    /// near-expiry threshold. `None` when the remaining lifetime is unknown.
    pub expiring_soon: Option<bool>,
}

/// Read a contract's storage entry by its `LedgerKey` (base64 XDR).
///
/// The instance key is NOT included automatically — the caller must supply
/// the complete `LedgerKey` via `--key-xdr`. ABI formatting is applied to the
/// returned `val` only when `--abi` is supplied; it is NOT used to discover
/// or construct the key.
pub async fn read_contract_state(
    client: &SorobanRpcClient,
    contract_id: &str,
    key_b64: &str,
    abi: Option<&sdkt_wasm::ContractSpec>,
) -> Result<StateReadResult, RpcError> {
    use stellar_xdr::WriteXdr;
    let decoded_key = sdkt_xdr::decode_ledger_key(key_b64)
        .map_err(|e| RpcError::Rpc(format!("Invalid LedgerKey: {e}")))?;
    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    decoded_key
        .write_xdr(&mut l)
        .map_err(|e| RpcError::Rpc(format!("Failed to re-encode LedgerKey: {e}")))?;
    let canonical_key_b64 = base64::engine::general_purpose::STANDARD.encode(&buf);

    let (entry, live_until_ledger) = read_ledger_entry_with_ttl(client, &canonical_key_b64).await?;

    let (entry_type, durability, value_json) = match &entry.data {
        LedgerEntryData::ContractData(cd) => {
            let durability_str = match cd.durability {
                stellar_xdr::ContractDataDurability::Persistent => "persistent",
                stellar_xdr::ContractDataDurability::Temporary => "temporary",
            };

            let value_json = match abi {
                Some(spec) => {
                    let decoded = sdkt_xdr::abi_decode::decode_with_abi(spec, &cd.val, None);
                    serde_json::json!({
                        "raw": decoded.raw,
                        "label": decoded.label,
                        "matched_type": decoded.matched_type,
                        "fields": decoded.fields,
                    })
                }
                None => {
                    let mut val_buf = Vec::new();
                    let mut val_cursor = std::io::Cursor::new(&mut val_buf);
                    let mut val_l =
                        stellar_xdr::Limited::new(&mut val_cursor, stellar_xdr::Limits::none());
                    cd.val
                        .write_xdr(&mut val_l)
                        .map_err(|e| RpcError::Rpc(format!("Failed to encode ScVal: {e}")))?;
                    let val_b64 = base64::engine::general_purpose::STANDARD.encode(&val_buf);
                    serde_json::json!({ "xdr": val_b64 })
                }
            };

            (
                "contract_data".into(),
                Some(durability_str.to_string()),
                value_json,
            )
        }
        LedgerEntryData::ContractCode(_cc) => (
            "contract_code".into(),
            None,
            serde_json::json!({ "note": "contract code entry" }),
        ),
        _ => {
            return Err(RpcError::Rpc(
                "Ledger entry is not a ContractData or ContractCode entry".into(),
            ));
        }
    };

    Ok(StateReadResult {
        contract_id: contract_id.to_string(),
        key: canonical_key_b64,
        entry_type,
        durability,
        value: value_json,
        live_until_ledger,
        ledgers_remaining: None,
        expiring_soon: None,
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

    #[test]
    fn test_parse_min_resource_fee_valid() {
        assert_eq!(parse_min_resource_fee("1000").unwrap(), 1000);
        assert_eq!(parse_min_resource_fee("0").unwrap(), 0);
    }

    #[test]
    fn test_parse_min_resource_fee_rejects_invalid() {
        assert!(parse_min_resource_fee("").is_err());
        assert!(parse_min_resource_fee("abc").is_err());
        assert!(parse_min_resource_fee("-1").is_err());
    }

    #[test]
    fn test_parse_min_resource_fee_rejects_overflow() {
        let huge = (u64::from(u32::MAX) + 1).to_string();
        assert!(parse_min_resource_fee(&huge).is_err());
    }

    #[test]
    fn test_collect_extend_keys_includes_instance() {
        let keys = collect_extend_keys(
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            &[],
        )
        .unwrap();
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn test_collect_extend_keys_dedupes_instance() {
        // Supplying the instance key explicitly should not duplicate it.
        let instance =
            instance_ledger_key("CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC")
                .unwrap();
        let keys = collect_extend_keys(
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            &[instance.clone(), instance],
        )
        .unwrap();
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn test_collect_extend_keys_rejects_invalid_extra() {
        let result = collect_extend_keys(
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            &["not-a-valid-key".to_string()],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_collect_extend_keys_rejects_bad_contract() {
        assert!(collect_extend_keys("not-a-contract", &[]).is_err());
    }

    #[test]
    fn test_extend_result_json_is_valid_and_contains_key_fields() {
        // Verifies the structured JSON the CLI emits for `--format json`.
        let result = ExtendResult {
            contract_id: "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC".into(),
            extend_to: 17280,
            footprint_keys: vec![
                "AAAABQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
                "AAAABQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAY=".into(),
            ],
            hash: "abc123".into(),
            status: "SUCCESS".into(),
            fee: 12345,
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(
            parsed["contract_id"],
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC"
        );
        assert_eq!(parsed["extend_to"], 17280);
        assert_eq!(parsed["hash"], "abc123");
        assert_eq!(parsed["status"], "SUCCESS");
        assert_eq!(parsed["fee"], 12345);

        let keys = parsed["footprint_keys"].as_array().unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn test_state_read_result_json_keeps_existing_keys_and_adds_ttl_context() {
        let base = StateReadResult {
            contract_id: "CABC".into(),
            key: "AAAA".into(),
            entry_type: "contract_data".into(),
            durability: Some("persistent".into()),
            value: serde_json::json!({ "xdr": "AAAA" }),
            live_until_ledger: None,
            ledgers_remaining: None,
            expiring_soon: None,
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&base).unwrap()).unwrap();

        // Pre-existing keys keep their names and shapes...
        assert_eq!(json["contract_id"], "CABC");
        assert_eq!(json["key"], "AAAA");
        assert_eq!(json["entry_type"], "contract_data");
        assert_eq!(json["durability"], "persistent");
        assert_eq!(json["value"]["xdr"], "AAAA");
        assert!(json["live_until_ledger"].is_null());
        // ...and the TTL context is additive only.
        assert!(json["ledgers_remaining"].is_null());
        assert!(json["expiring_soon"].is_null());

        let enriched = StateReadResult {
            live_until_ledger: Some(185_432),
            ledgers_remaining: Some(100_000),
            expiring_soon: Some(false),
            ..base
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&enriched).unwrap()).unwrap();
        assert_eq!(json["live_until_ledger"], 185_432);
        assert_eq!(json["ledgers_remaining"], 100_000);
        assert_eq!(json["expiring_soon"], false);
    }
}
