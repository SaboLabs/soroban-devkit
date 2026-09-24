//! Soroban transaction submission and polling engine.
//!
//! Provides `send_transaction`, `get_transaction_status`, and `poll_transaction`
//! to drive the full submit → poll → settle lifecycle, reusing
//! [`SorobanRpcClient`] for all HTTP/JSON-RPC transport.

use crate::{RpcError, SorobanRpcClient};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;
use stellar_xdr::{
    ContractEvent, Limited, Limits, ReadXdr, TransactionMeta, WriteXdr,
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
    /// Base64 `TransactionMeta` XDR from `getTransaction`, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_meta_xdr: Option<String>,
    /// Contract events (base64 `ContractEvent` XDR) extracted from `result_meta_xdr`
    /// on successful settlement. Empty when the invocation emitted none or meta
    /// could not be decoded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_ledger: Option<String>,
    /// Error code from sendTransaction (e.g. "tx_bad_auth"), when status == Failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Base64 TransactionResult XDR from sendTransaction, when status == Failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_result_xdr: Option<String>,
    /// Diagnostic events (base64 ContractEvent XDR) from sendTransaction, when status == Failed.
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


/// Encode a `ContractEvent` as base64 XDR.
fn encode_contract_event_b64(event: &ContractEvent) -> Option<String> {
    let mut buf = Vec::new();
    {
        let mut lim = Limited::new(&mut buf, Limits::none());
        event.write_xdr(&mut lim).ok()?;
    }
    Some(STANDARD.encode(buf))
}

/// Extract contract events from a base64 `TransactionMeta` XDR.
///
/// Supports:
/// - `TransactionMeta::V3` → `soroban_meta.events`
/// - `TransactionMeta::V4` → per-operation `events` plus top-level `TransactionEvent`s
///
/// Returns an empty list when meta is missing, undecodable, or contains no events.
/// Never includes diagnostic-only failure events (those stay on `diagnostic_events`).
pub fn extract_contract_events_from_meta_xdr(meta_xdr: &str) -> Vec<String> {
    let Ok(raw) = STANDARD.decode(meta_xdr.trim()) else {
        return Vec::new();
    };
    let mut cursor = std::io::Cursor::new(&raw);
    let mut lim = Limited::new(&mut cursor, Limits::none());
    let Ok(meta) = TransactionMeta::read_xdr(&mut lim) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    match meta {
        TransactionMeta::V3(v3) => {
            if let Some(soroban) = v3.soroban_meta {
                for ev in soroban.events.iter() {
                    if let Some(b64) = encode_contract_event_b64(ev) {
                        out.push(b64);
                    }
                }
            }
        }
        TransactionMeta::V4(v4) => {
            for op in v4.operations.iter() {
                for ev in op.events.iter() {
                    if let Some(b64) = encode_contract_event_b64(ev) {
                        out.push(b64);
                    }
                }
            }
            for te in v4.events.iter() {
                if let Some(b64) = encode_contract_event_b64(&te.event) {
                    out.push(b64);
                }
            }
        }
        TransactionMeta::V0(_) | TransactionMeta::V1(_) | TransactionMeta::V2(_) => {}
    }
    out
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
            result_meta_xdr: None,
            events: Vec::new(),
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
                let events = res
                    .result_meta_xdr
                    .as_deref()
                    .map(extract_contract_events_from_meta_xdr)
                    .unwrap_or_default();
                return Ok(SubmissionResult {
                    hash: hash.to_string(),
                    status,
                    result_xdr: res.result_xdr,
                    result_meta_xdr: res.result_meta_xdr,
                    events,
                    latest_ledger: res.latest_ledger,
                    error_code: None,
                    error_result_xdr: None,
                    diagnostic_events: Vec::new(),
                });
            }
            TransactionStatus::Failed => {
                return Ok(SubmissionResult {
                    hash: hash.to_string(),
                    status,
                    result_xdr: res.result_xdr,
                    result_meta_xdr: res.result_meta_xdr,
                    events: Vec::new(),
                    latest_ledger: res.latest_ledger,
                    error_code: None,
                    error_result_xdr: None,
                    diagnostic_events: Vec::new(),
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
            result_meta_xdr: None,
            events: Vec::new(),
            latest_ledger: Some("100".to_string()),
            error_code: None,
            error_result_xdr: None,
            diagnostic_events: Vec::new(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["status"], "Success");
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

    fn sample_contract_event() -> stellar_xdr::ContractEvent {
        use stellar_xdr::{
            ContractEventBody, ContractEventType, ContractEventV0, ExtensionPoint, ScVal, VecM,
        };
        stellar_xdr::ContractEvent {
            ext: ExtensionPoint::V0,
            contract_id: None,
            type_: ContractEventType::Contract,
            body: ContractEventBody::V0(ContractEventV0 {
                topics: VecM::default(),
                data: ScVal::Void,
            }),
        }
    }

    fn encode_meta_v3_with_events(events: Vec<stellar_xdr::ContractEvent>) -> String {
        use stellar_xdr::{
            ExtensionPoint, LedgerEntryChanges, ScVal, SorobanTransactionMeta,
            SorobanTransactionMetaExt, TransactionMeta, TransactionMetaV3, VecM, WriteXdr,
        };
        let meta = TransactionMeta::V3(TransactionMetaV3 {
            ext: ExtensionPoint::V0,
            tx_changes_before: LedgerEntryChanges::default(),
            operations: VecM::default(),
            tx_changes_after: LedgerEntryChanges::default(),
            soroban_meta: Some(SorobanTransactionMeta {
                ext: SorobanTransactionMetaExt::V0,
                events: events.try_into().unwrap(),
                return_value: ScVal::Void,
                diagnostic_events: VecM::default(),
            }),
        });
        let mut buf = Vec::new();
        {
            let mut lim = Limited::new(&mut buf, Limits::none());
            meta.write_xdr(&mut lim).unwrap();
        }
        STANDARD.encode(buf)
    }

    #[test]
    fn extract_events_from_meta_with_contract_events() {
        let event = sample_contract_event();
        let expected = encode_contract_event_b64(&event).unwrap();
        let meta = encode_meta_v3_with_events(vec![event]);
        let extracted = extract_contract_events_from_meta_xdr(&meta);
        assert_eq!(extracted, vec![expected]);
    }

    #[test]
    fn extract_events_from_meta_without_events_is_empty() {
        let meta = encode_meta_v3_with_events(Vec::new());
        assert!(extract_contract_events_from_meta_xdr(&meta).is_empty());
    }

    #[test]
    fn extract_events_from_invalid_meta_is_empty() {
        assert!(extract_contract_events_from_meta_xdr("not-valid-xdr!!!").is_empty());
    }

    #[test]
    fn status_response_deserializes_result_meta_xdr() {
        let raw = r#"{
            "status": "SUCCESS",
            "latestLedger": "100",
            "resultXdr": "AAAAres",
            "resultMetaXdr": "AAAAmeta",
            "error": null
        }"#;
        let resp: TransactionStatusResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.result_meta_xdr.as_deref(), Some("AAAAmeta"));
    }

    #[tokio::test]
    async fn poll_success_preserves_contract_events_from_meta() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let event = sample_contract_event();
        let expected = encode_contract_event_b64(&event).unwrap();
        let meta = encode_meta_v3_with_events(vec![event]);
        let meta_for_server = meta.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf).await.unwrap();
            let resp = format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg==","resultMetaXdr":"{meta_for_server}"}}}}"#
            );
            let http_resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp.len(),
                resp
            );
            sock.write_all(http_resp.as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
        });

        let client = SorobanRpcClient::new(&format!("http://{}", addr));
        let result = poll_transaction(&client, "deadbeef", &PollConfig::default())
            .await
            .unwrap();
        assert_eq!(result.status, TransactionStatus::Success);
        assert_eq!(result.events, vec![expected]);
        assert_eq!(result.result_meta_xdr.as_deref(), Some(meta.as_str()));
    }
}
