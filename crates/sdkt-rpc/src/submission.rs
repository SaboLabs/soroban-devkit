//! Soroban transaction submission and polling engine.
//!
//! Provides `send_transaction`, `get_transaction_status`, and `poll_transaction`
//! to drive the full submit → poll → settle lifecycle, reusing
//! [`SorobanRpcClient`] for all HTTP/JSON-RPC transport.

use crate::{RpcError, SorobanRpcClient};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Request payload for `sendTransaction`.
#[derive(Debug, Serialize)]
pub struct SendTransactionRequest {
    pub transaction: String,
}

/// Response from `sendTransaction`. `status` reflects the immediate
/// acceptance/processing state; final settlement requires polling.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SendTransactionResponse {
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub latest_ledger: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
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
    #[serde(default)]
    pub latest_ledger: Option<String>,
    #[serde(default)]
    pub latest_ledger_close_time: Option<String>,
    #[serde(default)]
    pub oldest_ledger: Option<String>,
    #[serde(default)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_ledger: Option<String>,
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
    if !wait {
        return Ok(SubmissionResult {
            hash,
            status: TransactionStatus::Pending,
            result_xdr: None,
            latest_ledger: None,
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
                    latest_ledger: res.latest_ledger,
                });
            }
            TransactionStatus::Failed => {
                return Ok(SubmissionResult {
                    hash: hash.to_string(),
                    status,
                    result_xdr: res.result_xdr,
                    latest_ledger: res.latest_ledger,
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
        assert_eq!(resp.error, None);
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
            latest_ledger: Some("100".to_string()),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["status"], "Success");
    }
}
