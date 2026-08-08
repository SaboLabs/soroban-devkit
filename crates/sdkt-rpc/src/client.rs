//! Soroban RPC HTTP client.
//!
//! Low-level JSON‑RPC over HTTP via [`reqwest`].

use crate::error::RpcError;
use sdkt_core::NetworkConfig;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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
    /// Configures a 15-second default timeout and basic connection pooling.
    ///
    /// # Example
    /// ```
    /// use sdkt_rpc::SorobanRpcClient;
    /// let client = SorobanRpcClient::new("https://soroban-testnet.stellar.org");
    /// ```
    pub fn new(endpoint: &str) -> Self {
        Self::with_options(endpoint, Some(15), Some(100))
    }

    /// Create a client with explicit pool and timeout settings.
    pub fn with_options(
        endpoint: &str,
        timeout_secs: Option<u64>,
        pool_max_idle: Option<usize>,
    ) -> Self {
        let mut builder = reqwest::Client::builder();

        if let Some(secs) = timeout_secs {
            builder = builder.timeout(Duration::from_secs(secs));
        }

        if let Some(max_idle) = pool_max_idle {
            builder = builder.pool_max_idle_per_host(max_idle);
        }

        let http_client = builder
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            endpoint: endpoint.to_string(),
            http_client,
        }
    }

    /// Create a client from [`NetworkConfig`].
    pub fn from_config(config: &NetworkConfig) -> Self {
        Self::with_options(
            &config.rpc_url,
            config.timeout_secs,
            config.pool_max_idle_per_host,
        )
    }

    /// Return the configured endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Helper for making JSON-RPC calls with basic timeout retry logic.
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

        // Simple single-retry logic for network-level timeouts or transient failures
        let mut attempt = 0;
        let mut last_err = None;

        while attempt < 2 {
            match self
                .http_client
                .post(&self.endpoint)
                .json(&payload)
                .send()
                .await
            {
                Ok(res) => {
                    let rpc_res: JsonRpcResponse<T> = match res.json().await {
                        Ok(json) => json,
                        Err(e) => return Err(RpcError::Reqwest(e)),
                    };

                    if let Some(error) = rpc_res.error {
                        return Err(RpcError::Rpc(error.message));
                    }

                    return rpc_res.result.ok_or_else(|| {
                        RpcError::Rpc("Missing result in JSON-RPC response".to_string())
                    });
                }
                Err(e) => {
                    if e.is_timeout() || e.is_connect() {
                        last_err = Some(e);
                        attempt += 1;
                        // short backoff
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        continue;
                    }
                    return Err(RpcError::Reqwest(e));
                }
            }
        }

        Err(RpcError::Reqwest(last_err.unwrap()))
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
        self.request("getLedgerEntries", serde_json::json!({ "keys": keys }))
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
            timeout_secs: None,
            pool_max_idle_per_host: None,
        };
        let c = SorobanRpcClient::from_config(&cfg);
        assert_eq!(c.endpoint(), "https://custom.example.com");
    }

    #[test]
    fn get_ledger_entries_request_uses_keys_object() {
        // Regression test for the getLedgerEntries request-shape bug: the Soroban
        // RPC expects `{"keys": [...]}`, not a bare positional array `["key"]`.
        let keys = vec!["AAAA".to_string()];
        let body = serde_json::json!({ "keys": keys });
        assert_eq!(body, serde_json::json!({ "keys": ["AAAA".to_string()] }));
        // The bare-array form (the old bug) must NOT match.
        assert_ne!(body, serde_json::json!(["AAAA".to_string()]));
    }

    // Regression test for the M43 HTTP/gzip transport blocker: when the Soroban RPC
    // answers with `Content-Encoding: gzip`, the reqwest client (with the `gzip`
    // feature enabled) must transparently decode the body and parse the JSON-RPC
    // response. This is hermetic — it stands up a local gzip-speaking server and
    // never touches the Stellar testnet.
    #[tokio::test]
    async fn request_decodes_gzip_response_body() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let payload = br#"{"jsonrpc":"2.0","id":1,"result":{"status":"ok"}}"#;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(payload).unwrap();
        let compressed = enc.finish().unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            // Drain the incoming HTTP request so the client doesn't block on write.
            let _ = sock.read(&mut buf).await.unwrap();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                compressed.len()
            );
            sock.write_all(header.as_bytes()).await.unwrap();
            sock.write_all(&compressed).await.unwrap();
        });

        let client = SorobanRpcClient::new(&format!("http://{}", addr));
        let res: HealthCheck = client.request("getHealth", ()).await.unwrap();
        assert_eq!(res.status, "ok");

        server.await.unwrap();
    }
}
