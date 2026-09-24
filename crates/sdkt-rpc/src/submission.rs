//! Soroban transaction submission and polling engine.
//!
//! Provides `send_transaction`, `get_transaction_status`, and `poll_transaction`
//! to drive the full submit → poll → settle lifecycle, reusing
//! [`SorobanRpcClient`] for all HTTP/JSON-RPC transport.

use crate::{RpcError, SorobanRpcClient};
use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;
use stellar_xdr::{
    DiagnosticEvent, InvokeHostFunctionResult, Limits, OperationResult, OperationResultTr, ReadXdr,
    TransactionMeta, TransactionResult, TransactionResultResult, WriteXdr,
};

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
    /// Base64 `TransactionMeta` XDR from the settled transaction, when the
    /// network returned one (the settled failure path keeps it so callers can
    /// inspect ledger/state changes behind a failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_meta_xdr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_ledger: Option<String>,
    /// Failure code when status == Failed.
    ///
    /// Mirrors `sendTransaction`'s `errorResult` on the immediate-rejection
    /// path, and on the settled path is derived from the network `error` field
    /// or from the settled `result_xdr` (see [`extract_failure_code`]). Never
    /// empty when a failure detail was available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Base64 TransactionResult XDR from sendTransaction, when status == Failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_result_xdr: Option<String>,
    /// Diagnostic events as base64 `ContractEvent` XDR, when status == Failed.
    ///
    /// Populated from `sendTransaction`'s `diagnosticEvents` on the
    /// immediate-rejection path, and derived from the settled
    /// `result_meta_xdr` on the polling path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostic_events: Vec<String>,
}

/// Derive a failure code for a settled `getTransaction` response.
///
/// Prefers the network-supplied `error` field (returned by RPC providers that
/// surface a failure reason) and otherwise decodes the settled `result_xdr`
/// as a `TransactionResult` and summarises its failure discriminator.
///
/// Codes are lower snake case: `tx_bad_seq`, `tx_insufficient_fee`,
/// `tx_failed`, or — when a Soroban host-function failure is identifiable —
/// `tx_failed:invoke_host_function_trapped`. Returns `None` when neither the
/// `error` field nor a decodable `result_xdr` yields a code.
pub fn extract_failure_code(res: &TransactionStatusResponse) -> Option<String> {
    if let Some(error) = res
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
    {
        return Some(error.to_string());
    }

    let result_xdr = res.result_xdr.as_deref()?;
    decode_transaction_result(result_xdr).map(|result| transaction_result_failure_code(&result))
}

/// Decode a base64 `TransactionResult` XDR payload.
///
/// Returns `None` for empty, malformed or truncated payloads so callers can
/// fall back to a generic failure message instead of failing outright.
pub fn decode_transaction_result(xdr_base64: &str) -> Option<TransactionResult> {
    let payload = xdr_base64.trim();
    if payload.is_empty() {
        return None;
    }
    TransactionResult::from_xdr_base64(payload, Limits::none()).ok()
}

/// Summarise a settled `TransactionResult` as a lower snake case failure code.
///
/// `TxFailed` results additionally report the failing Soroban host-function
/// code when one is present (e.g. `tx_failed:invoke_host_function_trapped`).
pub fn transaction_result_failure_code(result: &TransactionResult) -> String {
    let outer = snake_case(result.result.name());
    match &result.result {
        TransactionResultResult::TxFailed(operations) => host_function_failure_detail(operations)
            .map(|detail| format!("{outer}:{detail}"))
            .unwrap_or(outer),
        _ => outer,
    }
}

/// Diagnostic `ContractEvent` payloads (base64 XDR) carried by a settled
/// `TransactionMeta`.
///
/// Soroban settles with `TransactionMetaV3` (events under `soroban_meta`) or
/// `TransactionMetaV4` (events at the top level). Returns an empty vector for
/// meta that is absent, malformed, or of a non-Soroban version.
pub fn diagnostic_events_from_meta_xdr(meta_xdr: &str) -> Vec<String> {
    let payload = meta_xdr.trim();
    if payload.is_empty() {
        return Vec::new();
    }
    let Ok(meta) = TransactionMeta::from_xdr_base64(payload, Limits::none()) else {
        return Vec::new();
    };

    let events: Vec<&DiagnosticEvent> = match &meta {
        TransactionMeta::V3(v3) => v3
            .soroban_meta
            .as_ref()
            .map(|soroban| soroban.diagnostic_events.iter().collect())
            .unwrap_or_default(),
        TransactionMeta::V4(v4) => v4.diagnostic_events.iter().collect(),
        TransactionMeta::V0(_) | TransactionMeta::V1(_) | TransactionMeta::V2(_) => Vec::new(),
    };

    events
        .iter()
        .filter_map(|diagnostic| diagnostic.event.to_xdr_base64(Limits::none()).ok())
        .collect()
}

/// Failing Soroban host-function code carried by a `TxFailed` operation list.
///
/// Non-Soroban operations and successful invocations are skipped: they carry no
/// actionable host-function failure detail.
fn host_function_failure_detail(operations: &[OperationResult]) -> Option<String> {
    operations.iter().find_map(|operation| match operation {
        OperationResult::OpInner(OperationResultTr::InvokeHostFunction(
            InvokeHostFunctionResult::Success(_),
        )) => None,
        OperationResult::OpInner(OperationResultTr::InvokeHostFunction(other)) => {
            Some(format!("invoke_host_function_{}", snake_case(other.name())))
        }
        _ => None,
    })
}

/// `TxBadSeq` → `tx_bad_seq`.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
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
            result_meta_xdr: None,
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
            result_meta_xdr: None,
            latest_ledger: None,
            error_code: None,
            error_result_xdr: None,
            diagnostic_events: Vec::new(),
        });
    }
    poll_transaction(client, &hash, config).await
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
                    result_meta_xdr: res.result_meta_xdr,
                    latest_ledger: res.latest_ledger,
                    error_code: None,
                    error_result_xdr: None,
                    diagnostic_events: Vec::new(),
                });
            }
            TransactionStatus::Failed => {
                // Derive the diagnostics *before* the response fields are moved
                // into the result: the settled `result_xdr` / `result_meta_xdr`
                // carry the on-chain failure detail discarded previously.
                let error_code = extract_failure_code(&res);
                let diagnostic_events = res
                    .result_meta_xdr
                    .as_deref()
                    .map(diagnostic_events_from_meta_xdr)
                    .unwrap_or_default();
                return Ok(SubmissionResult {
                    hash: hash.to_string(),
                    status,
                    result_xdr: res.result_xdr,
                    result_meta_xdr: res.result_meta_xdr,
                    latest_ledger: res.latest_ledger,
                    error_code,
                    error_result_xdr: None,
                    diagnostic_events,
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
    use stellar_xdr::{
        ContractEvent, ContractEventBody, ContractEventType, ContractEventV0, ContractId,
        ExtensionPoint, Hash, LedgerEntryChanges, ScVal, SorobanTransactionMeta,
        SorobanTransactionMetaExt, TransactionMetaV3, TransactionResultExt, VecM,
    };
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
            result_meta_xdr: None,
            latest_ledger: Some("100".to_string()),
            error_code: None,
            error_result_xdr: None,
            diagnostic_events: Vec::new(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["status"], "Success");
        // Absent meta/diagnostics must not change the serialized shape.
        assert!(v.get("resultMetaXdr").is_none());
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

    // ---------- On-chain failure diagnostics (settled path) ----------

    /// A settled `TransactionResult` for a failed Soroban invocation
    /// (`txFAILED` carrying an `INVOKE_HOST_FUNCTION_TRAPPED` operation result).
    fn failed_invoke_result_xdr() -> String {
        let operations = VecM::try_from(vec![OperationResult::OpInner(
            OperationResultTr::InvokeHostFunction(InvokeHostFunctionResult::Trapped),
        )])
        .unwrap();
        TransactionResult {
            fee_charged: 100,
            result: TransactionResultResult::TxFailed(operations),
            ext: TransactionResultExt::V0,
        }
        .to_xdr_base64(Limits::none())
        .unwrap()
    }

    /// A settled `TransactionResult` for a transaction-level failure
    /// (`txBAD_SEQ`), which carries no operation results.
    fn bad_seq_result_xdr() -> String {
        TransactionResult {
            fee_charged: 100,
            result: TransactionResultResult::TxBadSeq,
            ext: TransactionResultExt::V0,
        }
        .to_xdr_base64(Limits::none())
        .unwrap()
    }

    fn diagnostic_event(data: ScVal) -> DiagnosticEvent {
        DiagnosticEvent {
            in_successful_contract_call: false,
            event: ContractEvent {
                ext: ExtensionPoint::V0,
                contract_id: Some(ContractId(Hash([7u8; 32]))),
                type_: ContractEventType::Diagnostic,
                body: ContractEventBody::V0(ContractEventV0 {
                    topics: VecM::default(),
                    data,
                }),
            },
        }
    }

    /// A settled Soroban `TransactionMetaV3` carrying two diagnostic events.
    fn failed_meta_xdr() -> String {
        let soroban_meta = SorobanTransactionMeta {
            ext: SorobanTransactionMetaExt::V0,
            events: VecM::default(),
            return_value: ScVal::Void,
            diagnostic_events: VecM::try_from(vec![
                diagnostic_event(ScVal::U32(1)),
                diagnostic_event(ScVal::U32(2)),
            ])
            .unwrap(),
        };
        TransactionMeta::V3(TransactionMetaV3 {
            ext: ExtensionPoint::V0,
            tx_changes_before: LedgerEntryChanges(VecM::default()),
            operations: VecM::default(),
            tx_changes_after: LedgerEntryChanges(VecM::default()),
            soroban_meta: Some(soroban_meta),
        })
        .to_xdr_base64(Limits::none())
        .unwrap()
    }

    #[test]
    fn test_snake_case_codes() {
        assert_eq!(snake_case("TxBadSeq"), "tx_bad_seq");
        assert_eq!(snake_case("TxFailed"), "tx_failed");
        assert_eq!(
            snake_case("InsufficientRefundableFee"),
            "insufficient_refundable_fee"
        );
        assert_eq!(snake_case("Success"), "success");
    }

    #[test]
    fn test_failure_code_from_settled_result_xdr() {
        let result = decode_transaction_result(&failed_invoke_result_xdr()).unwrap();
        assert_eq!(
            transaction_result_failure_code(&result),
            "tx_failed:invoke_host_function_trapped"
        );
    }

    #[test]
    fn test_failure_code_without_operation_detail() {
        let result = decode_transaction_result(&bad_seq_result_xdr()).unwrap();
        assert_eq!(transaction_result_failure_code(&result), "tx_bad_seq");
    }

    #[test]
    fn test_decode_transaction_result_rejects_bad_payloads() {
        assert!(decode_transaction_result("").is_none());
        assert!(decode_transaction_result("   ").is_none());
        assert!(decode_transaction_result("not-base64!!").is_none());
        // Truncated TransactionResult (fee_charged only) must not decode.
        assert!(decode_transaction_result("AAAAf////g==").is_none());
    }

    fn status_response(status: &str, result_xdr: Option<String>) -> TransactionStatusResponse {
        TransactionStatusResponse {
            status: status.to_string(),
            latest_ledger: None,
            latest_ledger_close_time: None,
            oldest_ledger: None,
            oldest_ledger_close_time: None,
            application_order: None,
            envelope_xdr: None,
            result_xdr,
            result_meta_xdr: None,
            error: None,
        }
    }

    #[test]
    fn test_extract_failure_code_prefers_network_error() {
        let mut res = status_response("FAILED", Some(failed_invoke_result_xdr()));
        res.error = Some("  tx_bad_auth  ".to_string());
        assert_eq!(extract_failure_code(&res).as_deref(), Some("tx_bad_auth"));
    }

    #[test]
    fn test_extract_failure_code_from_result_xdr() {
        let res = status_response("FAILED", Some(failed_invoke_result_xdr()));
        assert_eq!(
            extract_failure_code(&res).as_deref(),
            Some("tx_failed:invoke_host_function_trapped")
        );
    }

    #[test]
    fn test_extract_failure_code_none_without_details() {
        assert!(extract_failure_code(&status_response("FAILED", None)).is_none());
    }

    #[test]
    fn test_diagnostic_events_from_meta_xdr() {
        let events = diagnostic_events_from_meta_xdr(&failed_meta_xdr());
        assert_eq!(events.len(), 2);
        // Each entry is a base64 ContractEvent XDR that decodes back.
        let first = ContractEvent::from_xdr_base64(&events[0], Limits::none()).unwrap();
        assert_eq!(first.type_, ContractEventType::Diagnostic);
        assert_eq!(
            ContractEvent::from_xdr_base64(&events[1], Limits::none())
                .unwrap()
                .type_,
            ContractEventType::Diagnostic
        );
    }

    #[test]
    fn test_diagnostic_events_from_meta_xdr_tolerates_garbage() {
        assert!(diagnostic_events_from_meta_xdr("").is_empty());
        assert!(diagnostic_events_from_meta_xdr("not-xdr").is_empty());
    }

    /// Mock `getTransaction` server that answers a single request.
    fn mock_get_transaction(result_json: String) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = std::io::Read::read(&mut sock, &mut buf);
            let body = format!(r#"{{"jsonrpc":"2.0","id":1,"result":{result_json}}}"#);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = std::io::Write::write_all(&mut sock, resp.as_bytes());
        });
        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn test_poll_failed_propagates_error_code_meta_and_diagnostics() {
        let result_xdr = failed_invoke_result_xdr();
        let meta_xdr = failed_meta_xdr();
        let (url, handle) = mock_get_transaction(format!(
            r#"{{"status":"FAILED","latestLedger":"101","resultXdr":"{result_xdr}","resultMetaXdr":"{meta_xdr}"}}"#
        ));

        let client = SorobanRpcClient::new(&url);
        let result = poll_transaction(&client, "deadbeef", &PollConfig::default())
            .await
            .unwrap();
        handle.join().unwrap();

        assert_eq!(result.status, TransactionStatus::Failed);
        assert_eq!(
            result.error_code.as_deref(),
            Some("tx_failed:invoke_host_function_trapped")
        );
        // The settled result and meta XDRs are no longer discarded.
        assert_eq!(result.result_xdr.as_deref(), Some(result_xdr.as_str()));
        assert_eq!(result.result_meta_xdr.as_deref(), Some(meta_xdr.as_str()));
        assert_eq!(result.diagnostic_events.len(), 2);
    }

    #[tokio::test]
    async fn test_poll_failed_prefers_network_error_field() {
        let result_xdr = bad_seq_result_xdr();
        let (url, handle) = mock_get_transaction(format!(
            r#"{{"status":"FAILED","latestLedger":"101","resultXdr":"{result_xdr}","error":"tx_failed"}}"#
        ));

        let client = SorobanRpcClient::new(&url);
        let result = poll_transaction(&client, "deadbeef", &PollConfig::default())
            .await
            .unwrap();
        handle.join().unwrap();

        assert_eq!(result.status, TransactionStatus::Failed);
        assert_eq!(result.error_code.as_deref(), Some("tx_failed"));
        assert_eq!(result.result_xdr.as_deref(), Some(result_xdr.as_str()));
    }

    #[tokio::test]
    async fn test_poll_failed_without_details_still_reports_failure() {
        let (url, handle) =
            mock_get_transaction(r#"{"status":"FAILED","latestLedger":"101"}"#.to_string());

        let client = SorobanRpcClient::new(&url);
        let result = poll_transaction(&client, "deadbeef", &PollConfig::default())
            .await
            .unwrap();
        handle.join().unwrap();

        assert_eq!(result.status, TransactionStatus::Failed);
        assert_eq!(result.error_code, None);
        assert_eq!(result.result_xdr, None);
        assert!(result.diagnostic_events.is_empty());
    }

    #[tokio::test]
    async fn test_poll_success_keeps_result_meta_xdr() {
        let meta_xdr = failed_meta_xdr();
        let (url, handle) = mock_get_transaction(format!(
            r#"{{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg==","resultMetaXdr":"{meta_xdr}"}}"#
        ));

        let client = SorobanRpcClient::new(&url);
        let result = poll_transaction(&client, "deadbeef", &PollConfig::default())
            .await
            .unwrap();
        handle.join().unwrap();

        assert_eq!(result.status, TransactionStatus::Success);
        assert_eq!(result.error_code, None);
        assert_eq!(result.result_meta_xdr.as_deref(), Some(meta_xdr.as_str()));
    }
}
