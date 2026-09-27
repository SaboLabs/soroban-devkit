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

/// Fetches TTL info from the RPC node for a contract's instance entry plus any
/// explicit extra keys, and transforms it into a `TtlInfo` representation.
///
/// Validates `contract_id` and all `extra_keys` before any RPC call via
/// [`collect_extend_keys`].
pub async fn get_ttl_info_for_keys(
    client: &SorobanRpcClient,
    contract_id: &str,
    extra_keys: &[String],
) -> Result<TtlInfo, RpcError> {
    // Validate contract ID and all keys before contacting RPC.
    let keys = collect_extend_keys(contract_id, extra_keys)?;

    let ledger_info = client.get_ledger().await?;
    let current_ledger = ledger_info.sequence;

    let storage_resp = client.get_contract_storage(contract_id, &keys).await?;

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

/// Fetches TTL info from the RPC node and transforms it into a `TtlInfo` representation.
pub async fn get_ttl_info(
    client: &SorobanRpcClient,
    contract_id: &str,
) -> Result<TtlInfo, RpcError> {
    get_ttl_info_for_keys(client, contract_id, &[]).await
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

/// Result of a `RestoreFootprint` submission, or of a `--dry-run`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RestoreResult {
    pub contract_id: String,
    /// Ledger keys (base64 XDR) from the preamble footprint, read-only then read-write.
    pub footprint_keys: Vec<String>,
    /// Number of ledger keys the restore covers.
    pub restored_keys: usize,
    /// Transaction hash. `None` for a dry run, which submits nothing.
    pub hash: Option<String>,
    /// `SUCCESS` after a confirmed submission, `DRY_RUN` otherwise.
    pub status: String,
    /// Total transaction fee in stroops (inclusion fee + resource fee).
    pub fee: u32,
    /// `minResourceFee` reported by the restore preamble.
    pub min_resource_fee: u32,
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

/// Inclusion fee added on top of the resource fee, matching `extend_footprint`.
const RESTORE_INCLUSION_FEE: u32 = 100;

/// Turn a simulation's `restorePreamble` into `RestoreFootprint` parameters.
///
/// Pure: no network access. The preamble's `transactionData` is adopted as-is
/// (authoritative footprint and resource bounds), except that its
/// `resource_fee` is raised to `minResourceFee` if it is lower, so the fee
/// floor always holds. Returns the parameters and the reported
/// `minResourceFee`.
///
/// # Errors
/// Returns an actionable [`RpcError`] when there is no preamble (state is
/// live), when the preamble is malformed, or when its footprint is empty.
pub fn restore_params_from_simulation(
    simulation: &crate::simulate::SimulateResponse,
    source_account: &str,
    sequence: i64,
) -> Result<(sdkt_xdr::RestoreFootprintParams, u32), RpcError> {
    let Some(preamble) = &simulation.restore_preamble else {
        return Err(RpcError::Rpc(match &simulation.error {
            Some(err) => {
                format!("Nothing to restore: simulation failed without a restore preamble: {err}")
            }
            None => "Nothing to restore: simulation returned no restore preamble, \
                     so the contract state this invocation touches is live"
                .into(),
        }));
    };

    let mut soroban_data: SorobanTransactionData =
        sdkt_xdr::parse_soroban_transaction_data(&preamble.transaction_data).map_err(|e| {
            RpcError::Rpc(format!(
                "Restore preamble transactionData is malformed ({e}); \
                 re-run the simulation against a healthy RPC node"
            ))
        })?;

    let footprint = &soroban_data.resources.footprint;
    if footprint.read_only.is_empty() && footprint.read_write.is_empty() {
        return Err(RpcError::Rpc(
            "Restore preamble footprint is empty: no archived entries to restore".into(),
        ));
    }

    let min_resource_fee = parse_min_resource_fee(&preamble.min_resource_fee)?;
    if soroban_data.resource_fee < i64::from(min_resource_fee) {
        soroban_data.resource_fee = i64::from(min_resource_fee);
    }
    let resource_fee = u32::try_from(soroban_data.resource_fee).map_err(|_| {
        RpcError::Rpc(format!(
            "Restore preamble resource fee out of range: {}",
            soroban_data.resource_fee
        ))
    })?;

    let params = sdkt_xdr::RestoreFootprintParams {
        source_account: source_account.to_string(),
        sequence,
        fee: RESTORE_INCLUSION_FEE.saturating_add(resource_fee),
        soroban_data,
    };
    Ok((params, min_resource_fee))
}

/// Restore archived contract storage via `RestoreFootprint`.
///
/// Simulates `invocation_envelope` (typically the invocation that failed on
/// archived state) to obtain its `restorePreamble`, then builds a
/// `RestoreFootprint` transaction from the preamble's footprint and fee. With
/// `dry_run` the transaction is built but neither signed nor submitted;
/// otherwise it is signed and submitted, waiting for confirmation like
/// `extend_footprint`.
pub async fn restore_footprint(
    client: &SorobanRpcClient,
    contract_id: &str,
    invocation_envelope: &str,
    source_account: &str,
    signer: &sdkt_xdr::sign::Ed25519Signer,
    network: sdkt_xdr::sign::Network,
    dry_run: bool,
) -> Result<RestoreResult, RpcError> {
    let simulation = crate::simulate::simulate_transaction(client, invocation_envelope)
        .await
        .map_err(|e| RpcError::Rpc(format!("Restore simulation failed: {e}")))?;

    let sequence = crate::account::get_next_sequence(client, source_account).await?;
    let (params, min_resource_fee) =
        restore_params_from_simulation(&simulation, source_account, sequence)?;
    let footprint_keys = extract_footprint_keys(&params.soroban_data)?;

    let envelope = sdkt_xdr::build_restore_footprint_tx(&params)
        .map_err(|e| RpcError::Rpc(format!("Failed to build restore transaction: {e}")))?;

    let mut result = RestoreResult {
        contract_id: contract_id.to_string(),
        restored_keys: footprint_keys.len(),
        footprint_keys,
        hash: None,
        status: "DRY_RUN".into(),
        fee: params.fee,
        min_resource_fee,
    };
    if dry_run {
        return Ok(result);
    }

    let signing_opts = sdkt_xdr::sign::SigningOptions::with(network);
    let signed_envelope = sdkt_xdr::sign_transaction(&envelope, signer, &signing_opts)
        .map_err(|e| RpcError::Rpc(format!("Failed to sign restore transaction: {e}")))?;

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
            "Restore transaction failed: code={code} {diag}"
        )));
    }

    result.hash = Some(submission.hash);
    result.status = "SUCCESS".into();
    Ok(result)
}

/// Encode the footprint's ledger keys (read-only, then read-write) as base64 XDR.
fn extract_footprint_keys(soroban_data: &SorobanTransactionData) -> Result<Vec<String>, RpcError> {
    use stellar_xdr::{Limits, WriteXdr};
    let footprint = &soroban_data.resources.footprint;
    footprint
        .read_only
        .iter()
        .chain(footprint.read_write.iter())
        .map(|key| {
            key.to_xdr(Limits::none())
                .map(|raw| base64::engine::general_purpose::STANDARD.encode(raw))
                .map_err(|e| RpcError::Rpc(format!("Failed to encode LedgerKey: {e}")))
        })
        .collect()
}

/// Check whether a contract is live on-chain at `contract_id`.
///
/// Uses the cheapest possible existence probe: a single `getLedgerEntries`
/// call for the contract's instance singleton key (see [`instance_ledger_key`]).
/// Returns `Ok(true)` when the ledger responds with the instance entry and
/// `Ok(false)` when the entry is absent. This is the on-chain verification used
/// by `sdkt project deploy --skip-deployed` so a stale `.sdkt-deployments.json`
/// entry (contract deleted / never existed) does not cause a skip.
pub async fn contract_exists(
    client: &SorobanRpcClient,
    contract_id: &str,
) -> Result<bool, RpcError> {
    let key = instance_ledger_key(contract_id)?;
    let response = client.get_contract_storage("", &[key]).await?;
    Ok(!response.entries.is_empty())
}

/// Read a single ledger entry by its `LedgerKey` (base64 XDR).
///
/// Calls `getLedgerEntries` and returns the decoded `LedgerEntry`. If the
/// requested entry is absent, returns a clear error rather than an empty value.
pub async fn read_ledger_entry(
    client: &SorobanRpcClient,
    key_b64: &str,
) -> Result<stellar_xdr::LedgerEntry, RpcError> {
    let keys = vec![key_b64.to_string()];
    let response = client.get_contract_storage("", &keys).await?;

    if response.entries.is_empty() {
        return Err(RpcError::Rpc(
            "Ledger entry not found for the requested key".into(),
        ));
    }

    let entry_xdr = &response.entries[0].xdr;
    let entry_bytes = base64::engine::general_purpose::STANDARD
        .decode(entry_xdr.trim())
        .map_err(|e| RpcError::Rpc(format!("Failed to decode LedgerEntry XDR: {e}")))?;

    let mut cursor = std::io::Cursor::new(&entry_bytes);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    stellar_xdr::LedgerEntry::read_xdr(&mut l)
        .map_err(|e| RpcError::Rpc(format!("Failed to parse LedgerEntry: {e}")))
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

    let entry = read_ledger_entry(client, &canonical_key_b64).await?;

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
        live_until_ledger: None,
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

    #[tokio::test]
    async fn test_get_ttl_info_for_keys_rejects_invalid_key_before_rpc() {
        // Point to an invalid/unreachable address. If validation happens before RPC,
        // it fails immediately with invalid LedgerKey error, not a connection error.
        let client = SorobanRpcClient::new("http://127.0.0.1:1");
        let result = get_ttl_info_for_keys(
            &client,
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            &["not-a-valid-key".to_string()],
        )
        .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("invalid LedgerKey"));
    }

    #[tokio::test]
    async fn test_contract_exists_checks_on_chain_not_record_file() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        // Mock `getLedgerEntries` that returns an entry (contract live).
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        thread::spawn(move || {
            for conn in listener.incoming() {
                let mut sock = match conn {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let mut buf = [0u8; 16384];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.contains("getLedgerEntries") {
                    let entry = "AAAAAQAAAABpc25nAAAA".to_string();
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{entry}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                    )
                } else {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                        .to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let c = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
        let client = crate::SorobanRpcClient::new(&url);
        assert!(
            contract_exists(&client, c).await.unwrap(),
            "instance entry present at recorded address -> exists"
        );

        // Mock that serves an empty entries array -> recorded contract is gone.
        let listener2 = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr2 = listener2.local_addr().unwrap();
        let url2 = format!("http://{addr2}");
        thread::spawn(move || {
            for conn in listener2.incoming() {
                let mut sock = match conn {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let mut buf = [0u8; 16384];
                let n = sock.read(&mut buf).unwrap_or(0);
                let _req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = r#"{"jsonrpc":"2.0","id":1,"result":{"entries":[],"latestLedger":100}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let client2 = crate::SorobanRpcClient::new(&url2);
        assert!(
            !contract_exists(&client2, c).await.unwrap(),
            "empty entries at recorded address -> contract no longer exists"
        );
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
}

#[cfg(test)]
mod restore_tests {
    use super::*;
    use crate::simulate::SimulateResponse;
    use stellar_xdr::{
        ContractDataDurability, ContractId, Hash, LedgerFootprint, LedgerKey,
        LedgerKeyContractData, Limits, ScAddress, ScVal, SorobanResources,
        SorobanTransactionDataExt, VecM, WriteXdr,
    };

    const SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    fn preamble_data(read_write: Vec<LedgerKey>, resource_fee: i64) -> String {
        let data = SorobanTransactionData {
            ext: SorobanTransactionDataExt::V0,
            resources: SorobanResources {
                footprint: LedgerFootprint {
                    read_only: VecM::default(),
                    read_write: VecM::try_from(read_write).unwrap(),
                },
                instructions: 0,
                disk_read_bytes: 1_000,
                write_bytes: 1_000,
            },
            resource_fee,
        };
        base64::engine::general_purpose::STANDARD.encode(data.to_xdr(Limits::none()).unwrap())
    }

    fn instance_key() -> LedgerKey {
        LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([7; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        })
    }

    fn simulation(preamble: Option<(&str, &str)>, error: Option<&str>) -> SimulateResponse {
        let mut v = serde_json::json!({ "latestLedger": 100 });
        if let Some((data, fee)) = preamble {
            v["restorePreamble"] = serde_json::json!({
                "transactionData": data,
                "minResourceFee": fee,
            });
        }
        if let Some(err) = error {
            v["error"] = serde_json::json!(err);
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn adopts_preamble_footprint_unchanged() {
        let data = preamble_data(vec![instance_key()], 5_000);
        let sim = simulation(Some((&data, "5000")), None);
        let (params, min_fee) = restore_params_from_simulation(&sim, SOURCE, 42).unwrap();

        assert_eq!(min_fee, 5_000);
        assert_eq!(params.sequence, 42);
        assert_eq!(params.fee, RESTORE_INCLUSION_FEE + 5_000);
        let expected = sdkt_xdr::parse_soroban_transaction_data(&data).unwrap();
        assert_eq!(params.soroban_data, expected);
    }

    #[test]
    fn raises_resource_fee_to_min_resource_fee() {
        let data = preamble_data(vec![instance_key()], 1_000);
        let sim = simulation(Some((&data, "7500")), None);
        let (params, _) = restore_params_from_simulation(&sim, SOURCE, 1).unwrap();

        assert_eq!(params.soroban_data.resource_fee, 7_500);
        assert_eq!(params.fee, RESTORE_INCLUSION_FEE + 7_500);
    }

    #[test]
    fn missing_preamble_means_nothing_to_restore() {
        let err = restore_params_from_simulation(&simulation(None, None), SOURCE, 1).unwrap_err();
        assert!(err.to_string().contains("Nothing to restore"), "{err}");
        assert!(err.to_string().contains("live"), "{err}");
    }

    #[test]
    fn missing_preamble_surfaces_simulation_error() {
        let sim = simulation(None, Some("HostError: bad arg"));
        let err = restore_params_from_simulation(&sim, SOURCE, 1).unwrap_err();
        assert!(err.to_string().contains("HostError: bad arg"), "{err}");
    }

    #[test]
    fn empty_preamble_footprint_is_rejected() {
        let data = preamble_data(vec![], 100);
        let sim = simulation(Some((&data, "100")), None);
        let err = restore_params_from_simulation(&sim, SOURCE, 1).unwrap_err();
        assert!(err.to_string().contains("footprint is empty"), "{err}");
    }

    #[test]
    fn malformed_preamble_is_rejected() {
        let sim = simulation(Some(("not-xdr", "100")), None);
        let err = restore_params_from_simulation(&sim, SOURCE, 1).unwrap_err();
        assert!(err.to_string().contains("malformed"), "{err}");
    }

    #[test]
    fn footprint_keys_are_base64_ledger_keys() {
        let data = preamble_data(vec![instance_key()], 100);
        let parsed = sdkt_xdr::parse_soroban_transaction_data(&data).unwrap();
        let keys = extract_footprint_keys(&parsed).unwrap();
        let expected = base64::engine::general_purpose::STANDARD
            .encode(instance_key().to_xdr(Limits::none()).unwrap());
        assert_eq!(keys, vec![expected]);
    }
}
