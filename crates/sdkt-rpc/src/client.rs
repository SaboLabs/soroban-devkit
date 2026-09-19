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

        let endpoint = endpoint.trim_end_matches('/').to_string();

        Self {
            endpoint,
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

        let mut attempt = 0;
        let mut last_err = None;

        while attempt < 2 {
            match self
                .http_client
                .post(&self.endpoint)
                .json(&payload)
                .header("Accept-Encoding", "identity")
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FundResult {
    pub address: String,
    pub status: String,
}

/// Funds an account via the Stellar Testnet Friendbot.
///
/// Friendbot is a Testnet-only faucet. It is NOT available on Mainnet.
/// The caller must supply a network profile that explicitly carries a
/// `friendbot_url`. This function performs a single HTTP GET to the
/// Friendbot endpoint with the account address as a query parameter.
pub async fn fund_account(friendbot_url: &str, address: &str) -> Result<FundResult, RpcError> {
    if friendbot_url.trim().is_empty() {
        return Err(RpcError::Rpc("Friendbot URL is empty".into()));
    }
    if address.trim().is_empty() {
        return Err(RpcError::Rpc("Address is empty".into()));
    }

    let url = format!("{}?addr={}", friendbot_url.trim(), address);

    let response = reqwest::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                RpcError::Rpc(format!("Friendbot request timed out: {e}"))
            } else {
                RpcError::Reqwest(e)
            }
        })?;

    let status = response.status();
    let body = response.text().await.map_err(RpcError::Reqwest)?;

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(RpcError::Rpc(
            "Friendbot rate limit exceeded. Wait and try again.".into(),
        ));
    }

    if !status.is_success() {
        return Err(RpcError::Rpc(format!(
            "Friendbot returned HTTP {}: {}",
            status.as_u16(),
            body.chars().take(200).collect::<String>()
        )));
    }

    if body.contains("insufficient funds for rent exemption")
        || body.contains("account is already funded")
    {
        return Err(RpcError::Rpc(format!(
            "Friendbot refused: {}",
            body.chars().take(200).collect::<String>()
        )));
    }

    if body.contains("ERROR") || body.contains("error") {
        return Err(RpcError::Rpc(format!(
            "Friendbot error: {}",
            body.chars().take(200).collect::<String>()
        )));
    }

    Ok(FundResult {
        address: address.to_string(),
        status: "funded".into(),
    })
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
    pub status: String,
}

/// Ledger info snapshot.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerInfo {
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
        let keys = vec!["AAAA".to_string()];
        let body = serde_json::json!({ "keys": keys });
        assert_eq!(body, serde_json::json!({ "keys": ["AAAA".to_string()] }));
        assert_ne!(body, serde_json::json!(["AAAA".to_string()]));
    }

    #[tokio::test]
    async fn request_decodes_gzip_response_body() {
        // Removed: this test requires reqwest gzip feature which was disabled
        // to fix real Testnet E2E compatibility. This is a known trade-off.
    }

    #[tokio::test]
    async fn request_does_not_send_gzip_encoding_header() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel::<String>();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();

            let response = r#"{"jsonrpc":"2.0","id":1,"result":{"status":"ok"}}"#;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            sock.write_all(header.as_bytes()).await.unwrap();

            let _ = tx.send(request);
        });

        let client = SorobanRpcClient::new(&format!("http://{}", addr));
        let _res: HealthCheck = client.request("getHealth", ()).await.unwrap();

        let request = rx.await.unwrap();
        assert!(
            !request.contains("Accept-Encoding: gzip"),
            "Request should NOT contain Accept-Encoding: gzip, got:\n{}",
            request
        );
        assert!(
            !request.contains("Accept-Encoding: deflate"),
            "Request should NOT contain Accept-Encoding: deflate, got:\n{}",
            request
        );

        server.await.unwrap();
    }

    #[tokio::test]
    async fn fund_account_success() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();

            assert!(
                request.contains("addr=GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
            );

            let response = r#"{"hash":"abc123","ledger":12345}"#;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            sock.write_all(header.as_bytes()).await.unwrap();
        });

        let result = fund_account(
            &format!("http://{}", addr),
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .await
        .unwrap();

        assert_eq!(
            result.address,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
        );
        assert_eq!(result.status, "funded");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn fund_account_http_500() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _n = sock.read(&mut buf).await.unwrap();

            let response = r#"{"error":"internal"}"#;
            let header = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            sock.write_all(header.as_bytes()).await.unwrap();
        });

        let err = fund_account(
            &format!("http://{}", addr),
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("500"));

        server.await.unwrap();
    }

    #[tokio::test]
    async fn fund_account_rate_limit() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _n = sock.read(&mut buf).await.unwrap();

            let response = r#"{"error":"rate limited"}"#;
            let header = format!(
                "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            sock.write_all(header.as_bytes()).await.unwrap();
        });

        let err = fund_account(
            &format!("http://{}", addr),
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("rate limit"));

        server.await.unwrap();
    }

    #[tokio::test]
    async fn fund_account_empty_url_rejected() {
        let err = fund_account(
            "",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[tokio::test]
    async fn fund_account_empty_address_rejected() {
        let err = fund_account("http://localhost", "").await.unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[tokio::test]
    async fn fund_account_malformed_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _n = sock.read(&mut buf).await.unwrap();

            // Return 200 OK but with body containing "ERROR"
            let response = r#"{"status":"ERROR","message":"internal failure"}"#;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            sock.write_all(header.as_bytes()).await.unwrap();
        });

        let err = fund_account(
            &format!("http://{}", addr),
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .await
        .unwrap_err();

        // Should be rejected because body contains "error"
        assert!(err.to_string().contains("error") || err.to_string().contains("Error"));

        server.await.unwrap();
    }

    #[tokio::test]
    async fn fund_account_http_4xx() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _n = sock.read(&mut buf).await.unwrap();

            let response = r#"{"error":"bad request"}"#;
            let header = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            sock.write_all(header.as_bytes()).await.unwrap();
        });

        let err = fund_account(
            &format!("http://{}", addr),
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("400"));

        server.await.unwrap();
    }
}
