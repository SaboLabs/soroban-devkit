//! HTTP + JSON-RPC client for Soroban RPC endpoints.

use crate::RpcError;
use sdkt_core::NetworkConfig;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Health status returned by `getHealth`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthStatus {
    pub status: String,
}

/// Latest ledger metadata returned by `getLatestLedger`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerInfo {
    pub id: String,
    pub sequence: u32,
    pub protocol_version: u32,
}

/// A single raw ledger entry from `getLedgerEntries`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntryRaw {
    /// Base64 XDR of the `LedgerKey`.
    pub key: String,
    /// Base64 XDR of the `LedgerEntryData`.
    pub xdr: String,
    /// Ledger sequence at which this entry expires (state-archival TTL).
    #[serde(default)]
    pub live_until_ledger_seq: Option<u32>,
}

/// Response payload of `getLedgerEntries`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntriesResponse {
    #[serde(default)]
    pub entries: Vec<LedgerEntryRaw>,
    pub latest_ledger: u32,
}

#[derive(Serialize)]
struct JsonRpcRequest<P: Serialize> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: P,
}

#[derive(Deserialize)]
struct JsonRpcResponse<R> {
    result: Option<R>,
    error: Option<JsonRpcErrorBody>,
}

#[derive(Deserialize)]
struct JsonRpcErrorBody {
    message: String,
}

/// Soroban RPC client.
pub struct SorobanRpcClient {
    endpoint: String,
    http: reqwest::Client,
}

impl SorobanRpcClient {
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn from_config(config: &NetworkConfig) -> Self {
        Self::new(&config.rpc_url)
    }

    /// Generic JSON-RPC POST. Extracts `result` or maps the RPC error body.
    async fn rpc_call<P, R>(&self, method: &'static str, params: P) -> Result<R, RpcError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let body = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        };
        let resp = self.http.post(&self.endpoint).json(&body).send().await?;
        let envelope: JsonRpcResponse<R> = resp.json().await?;
        if let Some(err) = envelope.error {
            return Err(RpcError::Rpc(err.message));
        }
        envelope.result.ok_or(RpcError::InvalidParams)
    }

    /// `getHealth` — node liveness probe.
    pub async fn get_health(&self) -> Result<HealthStatus, RpcError> {
        self.rpc_call("getHealth", serde_json::json!({})).await
    }

    /// `getLatestLedger` — current ledger sequence + protocol version.
    pub async fn get_latest_ledger(&self) -> Result<LedgerInfo, RpcError> {
        self.rpc_call("getLatestLedger", serde_json::json!({}))
            .await
    }

    /// `getLedgerEntries` — fetch raw entries by base64-XDR ledger keys.
    pub async fn get_ledger_entries(
        &self,
        keys: &[String],
    ) -> Result<LedgerEntriesResponse, RpcError> {
        self.rpc_call("getLedgerEntries", serde_json::json!({ "keys": keys }))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_strips_trailing_slash() {
        let c = SorobanRpcClient::new("http://localhost:8000/");
        assert_eq!(c.endpoint, "http://localhost:8000");
    }

    #[test]
    fn from_config_uses_rpc_url() {
        let cfg = NetworkConfig {
            rpc_url: "https://soroban-testnet.stellar.org".to_string(),
            passphrase: "Test SDF Network ; September 2015".to_string(),
        };
        let c = SorobanRpcClient::from_config(&cfg);
        assert_eq!(c.endpoint, "https://soroban-testnet.stellar.org");
    }
}
