//! Soroban transaction submission and polling engine.
//!
//! Provides `send_transaction`, `get_transaction_status`, and `poll_transaction`
//! to drive the full submit → poll → settle lifecycle, reusing
//! [`SorobanRpcClient`] for all HTTP/JSON-RPC transport.

use crate::{RpcError, SorobanRpcClient};
use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;
use stellar_xdr::{
    InnerTransactionResultResult, InvokeHostFunctionResult, Limits, OperationResult,
    OperationResultTr, ReadXdr, TransactionMeta, TransactionResult, TransactionResultResult,
    WriteXdr,
};

/// Extract contract-event XDR entries from a settled `resultMetaXdr` value.
///
/// Soroban events live in the V3 transaction-level meta. V4 keeps operation
/// events alongside transaction-level events, whose stages place them before
/// all operations, after this transaction, or after all transactions.
pub fn extract_contract_events(result_meta_xdr: Option<&str>) -> Vec<String> {
    let Some(encoded) = result_meta_xdr else {
        return Vec::new();
    };
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return Vec::new();
    };
    let Ok(meta) = TransactionMeta::from_xdr(
        &raw,
        Limits {
            depth: 64,
            len: 1_048_576,
        },
    ) else {
        return Vec::new();
    };

    let events = match meta {
        TransactionMeta::V3(meta) => meta
            .soroban_meta
            .map(|soroban| soroban.events.into_iter().collect())
            .unwrap_or_default(),
        TransactionMeta::V4(meta) => {
            let mut events = Vec::new();
            let mut after_tx = Vec::new();
            let mut after_all_txs = Vec::new();

            for transaction_event in meta.events {
                match transaction_event.stage {
                    stellar_xdr::TransactionEventStage::BeforeAllTxs => {
                        events.push(transaction_event.event)
                    }
                    stellar_xdr::TransactionEventStage::AfterTx => {
                        after_tx.push(transaction_event.event)
                    }
                    stellar_xdr::TransactionEventStage::AfterAllTxs => {
                        after_all_txs.push(transaction_event.event)
                    }
                }
            }

            events.extend(
                meta.operations
                    .into_iter()
                    .flat_map(|operation| operation.events.into_iter()),
            );
            events.extend(after_tx);
            events.extend(after_all_txs);
            events
        }
        TransactionMeta::V0(_) | TransactionMeta::V1(_) | TransactionMeta::V2(_) => Vec::new(),
    };

    events
        .into_iter()
        .filter_map(|event| event.to_xdr(Limits::none()).ok())
        .map(|event| base64::engine::general_purpose::STANDARD.encode(event))
        .collect()
}

/// Helper to deserialize either a string or an integer into an Option<String>.
fn deserialize_optional_string_or_int<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(_) => return Err(Error::custom("expected string or number")),
        None => None,
    })
}

/// Request payload for `sendTransaction`.
#[derive(Debug, Serialize)]
pub struct SendTransactionRequest {
    pub transaction: String,
}

/// Response from `sendTransaction`. `status` reflects the immediate
/// acceptance/processing state; final settlement requires polling.
///
/// When `status` is `"ERROR"`, the `error_result`, `error_result_xdr`, and
/// `diagnostic_events` fields contain the network's rejection diagnostics.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SendTransactionResponse {
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub status: String,
    /// Accepts both string and integer values from Testnet.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    pub latest_ledger: Option<String>,
    /// Present only when status == "ERROR".
    #[serde(default)]
    pub latest_ledger_close_time: Option<String>,
    /// Base64 TransactionResult XDR present only when status == "ERROR".
    #[serde(default)]
    pub error_result_xdr: Option<String>,
    /// Diagnostic events (base64 ContractEvent XDR) present only when status == "ERROR".
    #[serde(default)]
    pub diagnostic_events: Vec<String>,
    /// Error code string (e.g. "tx_bad_auth", "tx_insufficient_balance") when status == "ERROR".
    #[serde(default)]
    pub error_result: Option<String>,
}

/// Terminal/transient status of a transaction on the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionStatus {
    Pending,
    Success,
    Failed,
    NotFound,
}

impl TransactionStatus {
    fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "success" => TransactionStatus::Success,
            "failed" | "error" => TransactionStatus::Failed,
            "not_found" => TransactionStatus::NotFound,
            _ => TransactionStatus::Pending,
        }
    }
}

/// Response from `getTransaction` during polling.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TransactionStatusResponse {
    #[serde(default)]
    pub status: String,
    /// Accepts both string and integer values from Testnet.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    pub latest_ledger: Option<String>,
    /// Accepts both string and integer values from Testnet.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    pub latest_ledger_close_time: Option<String>,
    /// Accepts both string and integer values from Testnet.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    pub oldest_ledger: Option<String>,
    /// Accepts both string and integer values from Testnet.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    pub oldest_ledger_close_time: Option<String>,
    #[serde(default)]
    pub application_order: Option<u64>,
    #[serde(default)]
    pub envelope_xdr: Option<String>,
    #[serde(default)]
    pub result_xdr: Option<String>,
    #[serde(default)]
    pub result_meta_xdr: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl TransactionStatusResponse {
    pub fn status_enum(&self) -> TransactionStatus {
        TransactionStatus::from_str(&self.status)
    }
}

/// Final result of the submission lifecycle.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionResult {
    pub hash: String,
    /// The settled status, or `Pending` if the caller did not wait.
    pub status: TransactionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_xdr: Option<String>,
    /// Raw base64 ContractEvent XDR emitted by a successful Soroban transaction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_ledger: Option<String>,
    /// Error code when status == Failed: the immediate-rejection code from
    /// sendTransaction, or the settled on-chain code from `getTransaction`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Base64 TransactionResult XDR when status == Failed (immediate
    /// rejection or settled failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_result_xdr: Option<String>,
    /// Diagnostic events (base64 ContractEvent XDR) when status == Failed
    /// (immediate rejection or settled failure).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostic_events: Vec<String>,
}

/// Configuration for transaction polling.
#[derive(Debug, Clone)]
pub struct PollConfig {
    pub timeout: Duration,
    pub interval: Duration,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            interval: Duration::from_secs(1),
        }
    }
}

/// Submit a signed transaction envelope (base64 XDR) to the network.
///
/// Reuses `SorobanRpcClient::request` for transport, timeout and a single
/// retry on transient network failures.
pub async fn send_transaction(
    client: &SorobanRpcClient,
    envelope: &str,
) -> Result<SendTransactionResponse, RpcError> {
    if envelope.trim().is_empty() {
        return Err(RpcError::Rpc("Transaction envelope is empty".to_string()));
    }
    let request = SendTransactionRequest {
        transaction: envelope.to_string(),
    };
    client.request("sendTransaction", request).await
}

/// Fetch the current status of a transaction by hash.
pub async fn get_transaction_status(
    client: &SorobanRpcClient,
    hash: &str,
) -> Result<TransactionStatusResponse, RpcError> {
    client
        .request("getTransaction", serde_json::json!({ "hash": hash }))
        .await
}

/// Submit then poll until the transaction settles (SUCCESS/FAILED) or times out.
///
/// - `status` on return reflects the final state reached.
/// - If `--wait` is not requested (`timeout == 0`), submits and returns
///   immediately with status `Pending`.
pub async fn submit_and_wait(
    client: &SorobanRpcClient,
    envelope: &str,
    wait: bool,
    config: &PollConfig,
) -> Result<SubmissionResult, RpcError> {
    let sent = send_transaction(client, envelope).await?;
    let hash = sent.hash.clone();

    // If the network rejected the transaction immediately (status == ERROR),
    // surface the diagnostics instead of waiting for a poll timeout.
    if sent.status.eq_ignore_ascii_case("ERROR") {
        return Ok(SubmissionResult {
            hash,
            status: TransactionStatus::Failed,
            result_xdr: None,
            events: Vec::new(),
            latest_ledger: sent.latest_ledger,
            error_code: sent.error_result,
            error_result_xdr: sent.error_result_xdr,
            diagnostic_events: sent.diagnostic_events,
        });
    }

    if !wait {
        return Ok(SubmissionResult {
            hash,
            status: TransactionStatus::Pending,
            result_xdr: None,
            events: Vec::new(),
            latest_ledger: None,
            error_code: None,
            error_result_xdr: None,
            diagnostic_events: Vec::new(),
        });
    }
    poll_transaction(client, &hash, config).await
}

/// Derive the most specific error code available for a settled FAILED
/// transaction: the network's own `error` string when present, otherwise the
/// deepest code decoded from the settled `result_xdr` `TransactionResult`.
/// Falls back to `"tx_failed"` so the field is never empty on this path.
fn settled_error_code(res: &TransactionStatusResponse) -> String {
    if let Some(err) = res.error.as_deref() {
        let err = err.trim();
        if !err.is_empty() {
            return err.to_string();
        }
    }
    if let Some(code) = res
        .result_xdr
        .as_deref()
        .and_then(decode_transaction_result_code)
    {
        return code;
    }
    "tx_failed".to_string()
}

/// Decode a base64 `TransactionResult` and describe its most specific
/// failure code (`tx_bad_auth`, `invoke_host_function_trapped`, ...).
fn decode_transaction_result_code(encoded: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let tx = TransactionResult::from_xdr(
        &raw,
        Limits {
            depth: 32,
            len: 1_048_576,
        },
    )
    .ok()?;
    Some(describe_transaction_result(&tx.result))
}

fn describe_transaction_result(result: &TransactionResultResult) -> String {
    match result {
        // A fee-bump wrapper carries no failure of its own; the inner
        // transaction result holds the operative code.
        TransactionResultResult::TxFeeBumpInnerSuccess(pair)
        | TransactionResultResult::TxFeeBumpInnerFailed(pair) => match &pair.result.result {
            InnerTransactionResultResult::TxFailed(ops) => describe_failed_ops(ops),
            InnerTransactionResultResult::TxSuccess(_) => "tx_success".to_string(),
            InnerTransactionResultResult::TxTooEarly => "tx_too_early".to_string(),
            InnerTransactionResultResult::TxTooLate => "tx_too_late".to_string(),
            InnerTransactionResultResult::TxMissingOperation => "tx_missing_operation".to_string(),
            InnerTransactionResultResult::TxBadSeq => "tx_bad_seq".to_string(),
            InnerTransactionResultResult::TxBadAuth => "tx_bad_auth".to_string(),
            InnerTransactionResultResult::TxInsufficientBalance => {
                "tx_insufficient_balance".to_string()
            }
            InnerTransactionResultResult::TxNoAccount => "tx_no_account".to_string(),
            InnerTransactionResultResult::TxInsufficientFee => "tx_insufficient_fee".to_string(),
            InnerTransactionResultResult::TxBadAuthExtra => "tx_bad_auth_extra".to_string(),
            InnerTransactionResultResult::TxInternalError => "tx_internal_error".to_string(),
            InnerTransactionResultResult::TxNotSupported => "tx_not_supported".to_string(),
            InnerTransactionResultResult::TxBadSponsorship => "tx_bad_sponsorship".to_string(),
            InnerTransactionResultResult::TxBadMinSeqAgeOrGap => {
                "tx_bad_min_seq_age_or_gap".to_string()
            }
            InnerTransactionResultResult::TxMalformed => "tx_malformed".to_string(),
            InnerTransactionResultResult::TxSorobanInvalid => "tx_soroban_invalid".to_string(),
            InnerTransactionResultResult::TxFrozenKeyAccessed => {
                "tx_frozen_key_accessed".to_string()
            }
        },
        TransactionResultResult::TxSuccess(_) => "tx_success".to_string(),
        TransactionResultResult::TxFailed(ops) => describe_failed_ops(ops),
        TransactionResultResult::TxTooEarly => "tx_too_early".to_string(),
        TransactionResultResult::TxTooLate => "tx_too_late".to_string(),
        TransactionResultResult::TxMissingOperation => "tx_missing_operation".to_string(),
        TransactionResultResult::TxBadSeq => "tx_bad_seq".to_string(),
        TransactionResultResult::TxBadAuth => "tx_bad_auth".to_string(),
        TransactionResultResult::TxInsufficientBalance => "tx_insufficient_balance".to_string(),
        TransactionResultResult::TxNoAccount => "tx_no_account".to_string(),
        TransactionResultResult::TxInsufficientFee => "tx_insufficient_fee".to_string(),
        TransactionResultResult::TxBadAuthExtra => "tx_bad_auth_extra".to_string(),
        TransactionResultResult::TxInternalError => "tx_internal_error".to_string(),
        TransactionResultResult::TxNotSupported => "tx_not_supported".to_string(),
        TransactionResultResult::TxBadSponsorship => "tx_bad_sponsorship".to_string(),
        TransactionResultResult::TxBadMinSeqAgeOrGap => "tx_bad_min_seq_age_or_gap".to_string(),
        TransactionResultResult::TxMalformed => "tx_malformed".to_string(),
        TransactionResultResult::TxSorobanInvalid => "tx_soroban_invalid".to_string(),
        TransactionResultResult::TxFrozenKeyAccessed => "tx_frozen_key_accessed".to_string(),
    }
}

/// Name the first non-success operation in a `TxFailed` result, drilling
/// into invoke-host-function results (the dominant Soroban failure shape).
fn describe_failed_ops(ops: &[OperationResult]) -> String {
    ops.iter()
        .filter_map(describe_operation_result)
        .find(|code| code != "invoke_host_function_success")
        .unwrap_or_else(|| "tx_failed".to_string())
}

fn describe_operation_result(op: &OperationResult) -> Option<String> {
    match op {
        OperationResult::OpInner(OperationResultTr::InvokeHostFunction(r)) => Some(match r {
            InvokeHostFunctionResult::Success(_) => "invoke_host_function_success".to_string(),
            InvokeHostFunctionResult::Malformed => "invoke_host_function_malformed".to_string(),
            InvokeHostFunctionResult::Trapped => "invoke_host_function_trapped".to_string(),
            InvokeHostFunctionResult::ResourceLimitExceeded => {
                "invoke_host_function_resource_limit_exceeded".to_string()
            }
            InvokeHostFunctionResult::EntryArchived => {
                "invoke_host_function_entry_archived".to_string()
            }
            InvokeHostFunctionResult::InsufficientRefundableFee => {
                "invoke_host_function_insufficient_refundable_fee".to_string()
            }
        }),
        // A non-invoke operation result carries no deeper machine code worth
        // surfacing; the caller falls back to the `tx_failed` level.
        OperationResult::OpInner(_) => None,
        OperationResult::OpBadAuth => Some("op_bad_auth".to_string()),
        OperationResult::OpNoAccount => Some("op_no_account".to_string()),
        OperationResult::OpNotSupported => Some("op_not_supported".to_string()),
        OperationResult::OpTooManySubentries => Some("op_too_many_subentries".to_string()),
        OperationResult::OpExceededWorkLimit => Some("op_exceeded_work_limit".to_string()),
        OperationResult::OpTooManySponsoring => Some("op_too_many_sponsoring".to_string()),
    }
}

/// Poll `getTransaction` until the transaction settles or the timeout elapses.
pub async fn poll_transaction(
    client: &SorobanRpcClient,
    hash: &str,
    config: &PollConfig,
) -> Result<SubmissionResult, RpcError> {
    let start = std::time::Instant::now();

    loop {
        let res = get_transaction_status(client, hash).await?;
        let status = res.status_enum();

        match status {
            TransactionStatus::Success => {
                return Ok(SubmissionResult {
                    hash: hash.to_string(),
                    status,
                    result_xdr: res.result_xdr,
                    events: extract_contract_events(res.result_meta_xdr.as_deref()),
                    latest_ledger: res.latest_ledger,
                    error_code: None,
                    error_result_xdr: None,
                    diagnostic_events: Vec::new(),
                });
            }
            TransactionStatus::Failed => {
                let error_code = settled_error_code(&res);
                return Ok(SubmissionResult {
                    hash: hash.to_string(),
                    status,
                    result_xdr: res.result_xdr.clone(),
                    events: Vec::new(),
                    latest_ledger: res.latest_ledger,
                    error_code: Some(error_code),
                    error_result_xdr: res.result_xdr,
                    diagnostic_events: extract_contract_events(res.result_meta_xdr.as_deref()),
                });
            }
            TransactionStatus::NotFound | TransactionStatus::Pending => {
                if start.elapsed() >= config.timeout {
                    return Err(RpcError::Rpc(format!(
                        "Transaction polling timed out after {}s (hash: {})",
                        config.timeout.as_secs(),
                        hash
                    )));
                }
                tokio::time::sleep(config.interval).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn test_status_parsing() {
        assert_eq!(
            TransactionStatus::from_str("SUCCESS"),
            TransactionStatus::Success
        );
        assert_eq!(
            TransactionStatus::from_str("failed"),
            TransactionStatus::Failed
        );
        assert_eq!(
            TransactionStatus::from_str("NOT_FOUND"),
            TransactionStatus::NotFound
        );
        assert_eq!(
            TransactionStatus::from_str("pending"),
            TransactionStatus::Pending
        );
    }

    #[test]
    fn test_send_request_serialization() {
        let req = SendTransactionRequest {
            transaction: "AAAAEnvelope===".to_string(),
        };
        let v = serde_json::to_value(req).unwrap();
        assert_eq!(v["transaction"], "AAAAEnvelope===");
    }

    #[test]
    fn test_send_response_deserialize() {
        let raw = r#"{"hash":"deadbeef","status":"PENDING","latestLedger":"100"}"#;
        let resp: SendTransactionResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.hash, "deadbeef");
        assert_eq!(resp.status, "PENDING");
        assert_eq!(resp.latest_ledger, Some("100".to_string()));
        assert_eq!(resp.error_result, None);
        assert!(resp.diagnostic_events.is_empty());
    }

    #[test]
    fn test_status_response_deserialize_full() {
        let raw = r#"{
            "status": "SUCCESS",
            "latestLedger": "100",
            "resultXdr": "AAAAres",
            "error": null
        }"#;
        let resp: TransactionStatusResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.status_enum(), TransactionStatus::Success);
        assert_eq!(resp.result_xdr, Some("AAAAres".to_string()));
    }

    #[test]
    fn test_submission_result_serialize() {
        let r = SubmissionResult {
            hash: "abc".to_string(),
            status: TransactionStatus::Success,
            result_xdr: Some("xdr".to_string()),
            events: Vec::new(),
            latest_ledger: Some("100".to_string()),
            error_code: None,
            error_result_xdr: None,
            diagnostic_events: Vec::new(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["status"], "Success");
        assert!(v.get("events").is_none());
    }

    #[test]
    fn extracts_raw_events_from_v3_transaction_meta() {
        use stellar_xdr::{ContractEvent, Limits, SorobanTransactionMeta, TransactionMetaV3};

        let event = ContractEvent::default();
        let meta = TransactionMeta::V3(TransactionMetaV3 {
            soroban_meta: Some(SorobanTransactionMeta {
                events: vec![event.clone()].try_into().unwrap(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(meta.to_xdr(Limits::none()).unwrap());
        let expected =
            base64::engine::general_purpose::STANDARD.encode(event.to_xdr(Limits::none()).unwrap());

        assert_eq!(extract_contract_events(Some(&encoded)), vec![expected]);
    }

    #[test]
    fn extracts_v4_transaction_and_operation_events_in_stage_order() {
        use stellar_xdr::{
            ContractEvent, ContractEventType, Limits, OperationMetaV2, TransactionEvent,
            TransactionEventStage, TransactionMetaV4,
        };

        let before = ContractEvent {
            type_: ContractEventType::System,
            ..Default::default()
        };
        let operation = ContractEvent {
            type_: ContractEventType::Contract,
            ..Default::default()
        };
        let after = ContractEvent {
            type_: ContractEventType::Diagnostic,
            ..Default::default()
        };
        let meta = TransactionMeta::V4(TransactionMetaV4 {
            operations: vec![OperationMetaV2 {
                events: vec![operation.clone()].try_into().unwrap(),
                ..Default::default()
            }]
            .try_into()
            .unwrap(),
            events: vec![
                TransactionEvent {
                    stage: TransactionEventStage::BeforeAllTxs,
                    event: before.clone(),
                },
                TransactionEvent {
                    stage: TransactionEventStage::AfterTx,
                    event: after.clone(),
                },
            ]
            .try_into()
            .unwrap(),
            ..Default::default()
        });
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(meta.to_xdr(Limits::none()).unwrap());
        let expected = [before, operation, after]
            .into_iter()
            .map(|event| {
                base64::engine::general_purpose::STANDARD
                    .encode(event.to_xdr(Limits::none()).unwrap())
            })
            .collect::<Vec<_>>();

        assert_eq!(extract_contract_events(Some(&encoded)), expected);
    }

    #[test]
    fn empty_or_invalid_meta_has_no_events() {
        assert!(extract_contract_events(None).is_empty());
        assert!(extract_contract_events(Some("not-xdr")).is_empty());
    }

    #[test]
    fn test_send_response_error_diagnostics_preserved() {
        let raw = r#"{
            "hash": "abc123",
            "status": "ERROR",
            "latestLedger": "100",
            "errorResult": "tx_bad_auth",
            "errorResultXdr": "AAAA",
            "diagnosticEvents": ["AAAAevent1", "AAAAevent2"]
        }"#;
        let resp: SendTransactionResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.hash, "abc123");
        assert_eq!(resp.status, "ERROR");
        assert_eq!(resp.error_result.as_deref(), Some("tx_bad_auth"));
        assert_eq!(resp.error_result_xdr.as_deref(), Some("AAAA"));
        assert_eq!(resp.diagnostic_events, vec!["AAAAevent1", "AAAAevent2"]);
    }

    #[test]
    fn test_send_response_pending_no_diagnostics() {
        let raw = r#"{
            "hash": "abc123",
            "status": "PENDING",
            "latestLedger": "100"
        }"#;
        let resp: SendTransactionResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.hash, "abc123");
        assert_eq!(resp.status, "PENDING");
        assert_eq!(resp.error_result, None);
        assert_eq!(resp.error_result_xdr, None);
        assert!(resp.diagnostic_events.is_empty());
    }

    #[tokio::test]
    async fn test_submit_and_wait_error_short_circuits_with_diagnostics() {
        // Build a mock HTTP server that returns ERROR status with diagnostics
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let _req = String::from_utf8_lossy(&buf[..n]).to_string();

            let resp = r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeef","status":"ERROR","latestLedger":"100","errorResult":"tx_bad_auth","errorResultXdr":"AAAA","diagnosticEvents":["AAAAevent"]}}"#;
            let http_resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp.len(),
                resp
            );
            use tokio::io::AsyncWriteExt;
            sock.write_all(http_resp.as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
        });

        let client = SorobanRpcClient::new(&format!("http://{}", addr));
        let result = submit_and_wait(&client, "AAAAEnvelope===", true, &PollConfig::default())
            .await
            .unwrap();

        assert_eq!(result.status, TransactionStatus::Failed);
        assert_eq!(result.hash, "deadbeef");
        assert_eq!(result.error_code.as_deref(), Some("tx_bad_auth"));
        assert_eq!(result.error_result_xdr.as_deref(), Some("AAAA"));
        assert_eq!(result.diagnostic_events, vec!["AAAAevent"]);
    }

    // ── #69: settled-failure diagnostics on the polling path ──────────

    fn failed_result_xdr(result: stellar_xdr::TransactionResultResult) -> String {
        use stellar_xdr::{TransactionResult, TransactionResultExt};
        let tx = TransactionResult {
            fee_charged: 100,
            result,
            ext: TransactionResultExt::V0,
        };
        base64::engine::general_purpose::STANDARD.encode(tx.to_xdr(Limits::none()).unwrap())
    }

    fn trapped_invoke_xdr() -> String {
        use stellar_xdr::{
            InvokeHostFunctionResult, OperationResult, OperationResultTr, TransactionResultResult,
        };
        failed_result_xdr(TransactionResultResult::TxFailed(
            vec![OperationResult::OpInner(
                OperationResultTr::InvokeHostFunction(InvokeHostFunctionResult::Trapped),
            )]
            .try_into()
            .unwrap(),
        ))
    }

    fn meta_with_one_event() -> (String, String) {
        use stellar_xdr::{ContractEvent, SorobanTransactionMeta, TransactionMetaV3};
        let event = ContractEvent::default();
        let meta = TransactionMeta::V3(TransactionMetaV3 {
            soroban_meta: Some(SorobanTransactionMeta {
                events: vec![event.clone()].try_into().unwrap(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let meta_b64 =
            base64::engine::general_purpose::STANDARD.encode(meta.to_xdr(Limits::none()).unwrap());
        let event_b64 =
            base64::engine::general_purpose::STANDARD.encode(event.to_xdr(Limits::none()).unwrap());
        (meta_b64, event_b64)
    }

    async fn poll_once_against(result_body: serde_json::Value) -> SubmissionResult {
        let body = result_body.to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let _req = String::from_utf8_lossy(&buf[..n]).to_string();

            let http_resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            use tokio::io::AsyncWriteExt;
            sock.write_all(http_resp.as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
        });

        let client = SorobanRpcClient::new(&format!("http://{}", addr));
        poll_transaction(
            &client,
            "abc123",
            &PollConfig {
                timeout: Duration::from_secs(5),
                interval: Duration::from_millis(10),
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn poll_failed_prefers_network_error_string() {
        let trapped = trapped_invoke_xdr();
        let res = poll_once_against(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "hash": "abc123", "status": "FAILED", "latestLedger": "100",
                "resultXdr": trapped, "resultMetaXdr": null, "error": "tx_failed"
            }
        }))
        .await;

        assert_eq!(res.status, TransactionStatus::Failed);
        assert_eq!(res.error_code.as_deref(), Some("tx_failed"));
        assert_eq!(res.error_result_xdr.as_deref(), Some(trapped.as_str()));
        assert!(res.diagnostic_events.is_empty());
    }

    #[tokio::test]
    async fn poll_failed_decodes_invoke_trap_and_keeps_meta_events() {
        let trapped = trapped_invoke_xdr();
        let (meta_b64, event_b64) = meta_with_one_event();
        let res = poll_once_against(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "hash": "abc123", "status": "FAILED", "latestLedger": "100",
                "resultXdr": trapped, "resultMetaXdr": meta_b64, "error": null
            }
        }))
        .await;

        assert_eq!(res.status, TransactionStatus::Failed);
        assert_eq!(
            res.error_code.as_deref(),
            Some("invoke_host_function_trapped")
        );
        assert_eq!(res.error_result_xdr.as_deref(), Some(trapped.as_str()));
        assert_eq!(res.result_xdr.as_deref(), Some(trapped.as_str()));
        assert_eq!(res.diagnostic_events, vec![event_b64]);
    }

    #[tokio::test]
    async fn poll_failed_without_any_diagnostics_still_names_the_failure() {
        let res = poll_once_against(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"hash": "abc123", "status": "FAILED", "latestLedger": "100"}
        }))
        .await;

        assert_eq!(res.status, TransactionStatus::Failed);
        assert_eq!(res.error_code.as_deref(), Some("tx_failed"));
        assert_eq!(res.error_result_xdr, None);
        assert!(res.diagnostic_events.is_empty());
    }

    #[test]
    fn settled_error_maps_top_level_result_codes() {
        use stellar_xdr::TransactionResultResult;
        let cases = [
            (TransactionResultResult::TxBadAuth, "tx_bad_auth"),
            (TransactionResultResult::TxBadSeq, "tx_bad_seq"),
            (
                TransactionResultResult::TxInsufficientBalance,
                "tx_insufficient_balance",
            ),
            (
                TransactionResultResult::TxSorobanInvalid,
                "tx_soroban_invalid",
            ),
        ];
        for (result, expected) in cases {
            let raw = serde_json::json!({
                "status": "FAILED",
                "resultXdr": failed_result_xdr(result),
                "error": null
            });
            let resp: TransactionStatusResponse = serde_json::from_value(raw).unwrap();
            assert_eq!(settled_error_code(&resp), expected);
        }
    }

    #[test]
    fn settled_error_names_fee_bump_inner_tx_code() {
        use stellar_xdr::{
            Hash, InnerTransactionResult, InnerTransactionResultPair, TransactionResultResult,
        };
        let pair = InnerTransactionResultPair {
            transaction_hash: Hash::default(),
            result: InnerTransactionResult {
                fee_charged: 100,
                result: stellar_xdr::InnerTransactionResultResult::TxBadAuth,
                ext: Default::default(),
            },
        };
        let raw = serde_json::json!({
            "status": "FAILED",
            "resultXdr": failed_result_xdr(TransactionResultResult::TxFeeBumpInnerFailed(pair)),
            "error": null
        });
        let resp: TransactionStatusResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(settled_error_code(&resp), "tx_bad_auth");
    }
}
