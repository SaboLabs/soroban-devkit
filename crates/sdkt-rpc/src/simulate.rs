//! Soroban transaction simulation.
//!
//! Provides `simulate_transaction` to dry-run a transaction envelope against
//! the network without submitting it, returning resource usage and auth entries.

use crate::{RpcError, SorobanRpcClient};
use serde::{Deserialize, Serialize};

/// Wrapper for the `simulateTransaction` RPC call.
#[derive(Debug, Serialize)]
pub struct SimulateTransactionRequest {
    pub transaction: String,
}

/// Result of simulating a single operation within the transaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SimulateOperationResult {
    /// List of base64 `SorobanAuthorizationEntry` XDR blobs (may be empty).
    #[serde(default)]
    pub auth: Vec<String>,
    /// Base64 XDR of the `SorobanTransactionMeta` entry for the operation.
    #[serde(default)]
    pub xdr: String,
}

/// Resource consumption report from a simulation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SimulateCost {
    pub cpu_insns: String,
    pub mem_bytes: String,
}

/// Full response from the `simulateTransaction` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SimulateResponse {
    /// Base64 XDR `SorobanTransactionData` returned for building the real transaction.
    #[serde(default)]
    pub transaction_data: String,
    /// Minimum resource fee in stroops.
    #[serde(default)]
    pub min_resource_fee: String,
    /// Per-operation results (present for host-function invocation transactions).
    #[serde(default)]
    pub results: Vec<SimulateOperationResult>,
    /// CPU and memory resource cost.
    #[serde(default)]
    pub cost: Option<SimulateCost>,
    /// Ledger sequence the simulation was run against.
    #[serde(default)]
    pub latest_ledger: Option<String>,
    /// Diagnostic events emitted during simulation.
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
    /// Optional simulation error (e.g. host function failure). A populated
    /// `error` field indicates the simulation did not fully succeed.
    #[serde(default)]
    pub error: Option<String>,
}

/// Runs `simulateTransaction` against the network for a base64 envelope XDR.
///
/// Follows the existing RPC module pattern: a standalone async function that
/// reuses `SorobanRpcClient::request` for JSON-RPC transport, timeout and retry.
pub async fn simulate_transaction(
    client: &SorobanRpcClient,
    envelope: &str,
) -> Result<SimulateResponse, RpcError> {
    if envelope.trim().is_empty() {
        return Err(RpcError::Rpc("Transaction envelope is empty".to_string()));
    }

    let request = SimulateTransactionRequest {
        transaction: envelope.to_string(),
    };

    let response: SimulateResponse = client.request("simulateTransaction", request).await?;

    Ok(response)
}

/// Validates that a transaction envelope is non-empty. Exposed as a pure helper
/// so the guard can be unit-tested without a live `SorobanRpcClient`.
pub fn validate_envelope(envelope: &str) -> Result<(), RpcError> {
    if envelope.trim().is_empty() {
        return Err(RpcError::Rpc("Transaction envelope is empty".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let request = SimulateTransactionRequest {
            transaction: "AAAAEnvelope===".to_string(),
        };
        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["transaction"], "AAAAEnvelope===");
    }

    #[test]
    fn test_response_deserialize_full() {
        let raw = r#"{
            "transactionData": "AAAAAdata",
            "minResourceFee": "1000",
            "results": [
                {"auth": ["AAAAauth1"], "xdr": "AAAAxdr1"}
            ],
            "cost": {"cpuInsns": "5000", "memBytes": "2048"},
            "latestLedger": "12345",
            "events": [{"type": "diagnostic"}]
        }"#;
        let response: SimulateResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(response.transaction_data, "AAAAAdata");
        assert_eq!(response.min_resource_fee, "1000");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].auth, vec!["AAAAauth1".to_string()]);
        assert_eq!(response.results[0].xdr, "AAAAxdr1");
        let cost = response.cost.unwrap();
        assert_eq!(cost.cpu_insns, "5000");
        assert_eq!(cost.mem_bytes, "2048");
        assert_eq!(response.latest_ledger, Some("12345".to_string()));
        assert_eq!(response.error, None);
    }

    #[test]
    fn test_response_deserialize_minimal() {
        // Minimal valid response with only transactionData present.
        let raw = r#"{"transactionData": "AAAAAdata"}"#;
        let response: SimulateResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(response.min_resource_fee, "");
        assert!(response.results.is_empty());
        assert!(response.cost.is_none());
        assert!(response.events.is_empty());
        assert_eq!(response.error, None);
    }

    #[test]
    fn test_response_deserialize_error_field() {
        let raw = r#"{
            "transactionData": "",
            "error": "host function failed"
        }"#;
        let response: SimulateResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(response.error, Some("host function failed".to_string()));
    }

    #[test]
    fn test_response_deserialize_malformed() {
        let raw = r#"{"transactionData": }"#;
        assert!(serde_json::from_str::<SimulateResponse>(raw).is_err());
    }

    #[test]
    fn test_empty_envelope_rejected() {
        assert!(validate_envelope("   ").is_err());
        assert!(validate_envelope("").is_err());
        assert!(validate_envelope("AAAAEnvelope===").is_ok());
    }
}
