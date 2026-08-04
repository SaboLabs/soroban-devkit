//! Soroban RPC HTTP client.
//!
//! Low-level JSON‑RPC over HTTP via [`reqwest`].

use crate::error::RpcError;
use sdkt_core::NetworkConfig;
use serde::{Deserialize, Serialize};

/// Soroban RPC HTTP client.
///
/// Holds the RPC endpoint URL and a reusable HTTP client.
#[derive(Clone)]
pub struct SorobanRpcClient {
    /// Base URL of the Soroban RPC endpoint (e.g. `https://soroban-testnet.stellar.org`).
    endpoint: String,
    http_client: reqwest::Client,
}

impl SorobanRpcClient {
    /// Create a client from an explicit endpoint URL.
    ///
    /// # Example
    /// ```
    /// use sdkt_rpc::SorobanRpcClient;
    /// let client = SorobanRpcClient::new("https://soroban-testnet.stellar.org");
    /// ```
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    /// Create a client from [`NetworkConfig`].
    pub fn from_config(config: &NetworkConfig) -> Self {
        Self::new(&config.rpc_url)
    }

    /// Return the configured endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Helper for making JSON-RPC calls.
    pub async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T, RpcError> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let res = self
            .http_client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await?;

        let rpc_res: JsonRpcResponse<T> = res.json().await?;

        if let Some(error) = rpc_res.error {
            return Err(RpcError::Rpc(error.message));
        }

        rpc_res
            .result
            .ok_or_else(|| RpcError::Rpc("Missing result in JSON-RPC response".to_string()))
    }

    /// Check the health of the Soroban RPC node.
    pub async fn get_health(&self) -> Result<HealthCheck, RpcError> {
        self.request("getHealth", ()).await
    }

    /// Get the latest ledger info from the Soroban RPC node.
    pub async fn get_ledger(&self) -> Result<LedgerInfo, RpcError> {
        self.request("getLatestLedger", ()).await
    }

    /// Get contract storage entries.
    pub async fn get_contract_storage(
        &self,
        _contract_id: &str,
        keys: &[String],
    ) -> Result<StorageResponse, RpcError> {
        self.request("getLedgerEntries", serde_json::json!([keys]))
            .await
    }
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    message: String,
}

/// Health check response from the node.
#[derive(Debug, Deserialize, PartialEq)]
pub struct HealthCheck {
    /// Status string (e.g. `"ok"`, `"error"`).
    pub status: String,
}

/// Ledger info snapshot.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerInfo {
    /// Current ledger sequence number.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub protocol_version: u32,
    pub sequence: u32,
}

/// Storage response payload.
#[derive(Debug, Deserialize, PartialEq)]
pub struct StorageResponse {
    pub entries: Vec<LedgerEntryResult>,
    #[serde(rename = "latestLedger")]
    pub latest_ledger: u32,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntryResult {
    pub key: String,
    pub xdr: String,
    pub last_modified_ledger_seq: u32,
    pub live_until_ledger_seq: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkt_core::NetworkConfig;

    #[test]
    fn new_sets_endpoint() {
        let c = SorobanRpcClient::new("http://localhost:8000");
        assert_eq!(c.endpoint(), "http://localhost:8000");
    }

    #[test]
    fn from_config_sets_endpoint() {
        let cfg = NetworkConfig {
            rpc_url: "https://custom.example.com".to_string(),
            passphrase: "test".to_string(),
        };
        let c = SorobanRpcClient::from_config(&cfg);
        assert_eq!(c.endpoint(), "https://custom.example.com");
    }
}
