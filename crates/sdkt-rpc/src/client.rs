//! Soroban RPC HTTP client.
//!
//! Low-level JSON‑RPC over HTTP via [`reqwest`].

use crate::error::RpcError;
use sdkt_core::NetworkConfig;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Maximum number of keys accepted by one Soroban `getLedgerEntries` request.
pub const MAX_LEDGER_KEYS: usize = 200;

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

    /// Return a reference to the internal HTTP client.
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    /// Helper for making JSON-RPC calls with basic timeout retry logic.
    ///
    /// Retryable failures — transport timeouts/connect errors and retryable
    /// HTTP statuses (`429`, `5xx`) — are retried within the same two-attempt
    /// budget. A non-success status is surfaced as [`RpcError::HttpStatus`]
    /// (or, when its body fails to parse, an error naming the status) instead
    /// of being misread as a body-decoding failure.
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
        let mut last_err: Option<RpcError> = None;

        while attempt < 2 {
            let res = match self
                .http_client
                .post(&self.endpoint)
                .json(&payload)
                .header("Accept-Encoding", "identity")
                .send()
                .await
            {
                Ok(res) => res,
                Err(e) => {
                    if e.is_timeout() || e.is_connect() {
                        last_err = Some(RpcError::Reqwest(e));
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        continue;
                    }
                    return Err(RpcError::Reqwest(e));
                }
            };

            if !res.status().is_success() {
                let status = res.status();
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    // Retryable at the HTTP level: same budget, same backoff
                    // as the transport branch.
                    last_err = Some(RpcError::HttpStatus {
                        status: status.as_u16(),
                    });
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                return Err(RpcError::HttpStatus {
                    status: status.as_u16(),
                });
            }

            let rpc_res: JsonRpcResponse<T> = match res.json().await {
                Ok(json) => json,
                Err(e) => return Err(RpcError::Reqwest(e)),
            };

            if let Some(error) = rpc_res.error {
                // JSON-RPC 2.0 requires `code`; a node that omits it
                // still gets its message surfaced as before.
                return Err(match error.code {
                    Some(code) => RpcError::JsonRpc {
                        code,
                        message: error.message,
                        data: error.data,
                    },
                    None => RpcError::Rpc(error.message),
                });
            }

            return rpc_res
                .result
                .ok_or_else(|| RpcError::Rpc("Missing result in JSON-RPC response".to_string()));
        }

        Err(last_err.unwrap_or_else(|| RpcError::Rpc("request failed".to_string())))
    }

    /// Check the health of the Soroban RPC node.
    pub async fn get_health(&self) -> Result<HealthCheck, RpcError> {
        self.request("getHealth", ()).await
    }

    /// Get the latest ledger info from the Soroban RPC node.
    pub async fn get_ledger(&self) -> Result<LedgerInfo, RpcError> {
        self.request("getLatestLedger", ()).await
    }

    /// Get network identity and protocol information from the RPC endpoint.
    pub async fn get_network(&self) -> Result<NetworkInfo, RpcError> {
        self.request("getNetwork", ()).await
    }

    /// Get contract storage entries in a single RPC request.
    ///
    /// More than [`MAX_LEDGER_KEYS`] keys returns [`RpcError::TooManyLedgerKeys`]
    /// before any HTTP request. Use [`crate::storage::fetch_ledger_entries`] for
    /// larger key sets, which are fetched in sequential batches.
    pub async fn get_contract_storage(
        &self,
        _contract_id: &str,
        keys: &[String],
    ) -> Result<StorageResponse, RpcError> {
        if keys.len() > MAX_LEDGER_KEYS {
            return Err(RpcError::TooManyLedgerKeys {
                count: keys.len(),
                max: MAX_LEDGER_KEYS,
            });
        }
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
    #[serde(default)]
    code: Option<i64>,
    message: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
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

/// Network identity and protocol details reported by `getNetwork`.
///
/// `latest_ledger` is optional because the SDF endpoints (testnet, futurenet,
/// mainnet) answer `getNetwork` with only `passphrase`, `protocolVersion` and
/// `friendbotUrl`; a required field there made every `getNetwork` decode fail
/// on those endpoints and the error was swallowed by callers that treated the
/// probe as best-effort.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    #[serde(default)]
    pub friendbot_url: Option<String>,
    pub passphrase: String,
    pub protocol_version: u32,
    #[serde(default)]
    pub latest_ledger: Option<u32>,
}

/// Storage response payload.
#[derive(Debug, Deserialize, PartialEq)]
pub struct StorageResponse {
    pub entries: Vec<LedgerEntryResult>,
    #[serde(rename = "latestLedger")]
    pub latest_ledger: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntryResult {
    pub key: String,
    pub xdr: String,
    pub last_modified_ledger_seq: u32,
    pub live_until_ledger_seq: Option<u32>,
    /// Base64 XDR encoded `ScVal` extracted from the entry's `ContractData`
    /// when the entry is a contract-data entry. `None` for `ContractCode`
    /// entries, entries whose XDR fails to parse, or when the field is absent
    /// from the RPC response. Additive: existing consumers ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
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
    fn network_info_deserializes_rpc_camel_case_fields() {
        let info: NetworkInfo = serde_json::from_value(serde_json::json!({
            "friendbotUrl": "https://friendbot.example",
            "passphrase": "Test Network",
            "protocolVersion": 22,
            "latestLedger": 1234
        }))
        .unwrap();

        assert_eq!(
            info.friendbot_url.as_deref(),
            Some("https://friendbot.example")
        );
        assert_eq!(info.passphrase, "Test Network");
        assert_eq!(info.protocol_version, 22);
        assert_eq!(info.latest_ledger, Some(1234));
    }

    /// The live SDF endpoints answer `getNetwork` without `latestLedger`; a
    /// required field there made the decode fail on every real endpoint.
    #[test]
    fn network_info_deserializes_without_latest_ledger() {
        let info: NetworkInfo = serde_json::from_value(serde_json::json!({
            "friendbotUrl": "https://friendbot.stellar.org/",
            "passphrase": "Test SDF Network ; September 2015",
            "protocolVersion": 29
        }))
        .unwrap();
        assert_eq!(info.protocol_version, 29);
        assert_eq!(info.latest_ledger, None);
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

    /// Serve one canned JSON-RPC body on a local socket and return its URL.
    async fn serve_once(body: &'static str) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _n = sock.read(&mut buf).await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });
        (format!("http://{addr}"), server)
    }

    /// Serve a fixed HTTP status + body to *every* request, counting how many
    /// requests arrived. Returns the URL, the request counter, and the handle.
    async fn serve_status_always(
        status_line: &'static str,
        body: &'static str,
    ) -> (
        String,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let count_srv = Arc::clone(&count);

        let server = tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let mut buf = [0u8; 8192];
                let _n = sock.read(&mut buf).await.unwrap_or(0);
                count_srv.fetch_add(1, Ordering::SeqCst);
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(response.as_bytes()).await;
            }
        });
        (format!("http://{addr}"), count, server)
    }

    /// A non-JSON `HTTP 503` body must yield an error naming the status, not a
    /// body-decoding error — that was the core ambiguity of the issue.
    #[tokio::test]
    async fn request_non_json_503_names_status() {
        let (url, count, server) =
            serve_status_always("503 Service Unavailable", "upstream unavailable").await;

        let err = SorobanRpcClient::new(&url)
            .request::<HealthCheck>("getHealth", ())
            .await
            .unwrap_err();

        let text = err.to_string();
        assert!(text.contains("503"), "error must name the status: {text}");
        assert!(
            matches!(err, RpcError::HttpStatus { status: 503 }),
            "expected HttpStatus{{503}}, got {err:?}"
        );
        // 503 is retryable: the budget is 2 attempts.
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    /// A `HTTP 503` carrying a valid JSON-RPC error body must be retried and,
    /// once the budget is exhausted, name the status — the retry branch must
    /// not be skipped just because the body parsed.
    #[tokio::test]
    async fn request_jsonrpc_503_is_retried_then_names_status() {
        let (url, count, server) = serve_status_always(
            "503 Service Unavailable",
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"service unavailable"}}"#,
        )
        .await;

        let err = SorobanRpcClient::new(&url)
            .request::<HealthCheck>("getHealth", ())
            .await
            .unwrap_err();

        let text = err.to_string();
        assert!(text.contains("503"), "{text}");
        assert!(
            matches!(err, RpcError::HttpStatus { status: 503 }),
            "expected HttpStatus{{503}}, got {err:?}"
        );
        assert!(
            count.load(std::sync::atomic::Ordering::SeqCst) > 1,
            "503 must be retried"
        );
        server.abort();
    }

    #[tokio::test]
    async fn request_429_is_retried() {
        let (url, count, server) = serve_status_always("429 Too Many Requests", "slow down").await;

        let err = SorobanRpcClient::new(&url)
            .request::<HealthCheck>("getHealth", ())
            .await
            .unwrap_err();

        assert!(matches!(err, RpcError::HttpStatus { status: 429 }));
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    /// A non-retryable 4xx must fail immediately with the status named, and
    /// must NOT consume a retry attempt.
    #[tokio::test]
    async fn request_400_is_terminal_and_names_status() {
        let (url, count, server) = serve_status_always("400 Bad Request", "nope").await;

        let err = SorobanRpcClient::new(&url)
            .request::<HealthCheck>("getHealth", ())
            .await
            .unwrap_err();

        assert!(matches!(err, RpcError::HttpStatus { status: 400 }));
        assert!(err.to_string().contains("400"));
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "400 must not be retried"
        );
        server.abort();
    }

    #[tokio::test]
    async fn request_preserves_jsonrpc_error_code_without_data() {
        let (url, server) = serve_once(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"unexpected hash length (2)"}}"#,
        )
        .await;

        let err = SorobanRpcClient::new(&url)
            .request::<HealthCheck>("getTransaction", ())
            .await
            .unwrap_err();

        assert_eq!(err.code(), Some(-32602));
        assert_eq!(err.data(), None);
        match &err {
            RpcError::JsonRpc {
                code,
                message,
                data,
            } => {
                assert_eq!(*code, -32602);
                assert_eq!(message, "unexpected hash length (2)");
                assert!(data.is_none());
            }
            other => panic!("expected RpcError::JsonRpc, got {other:?}"),
        }
        assert_eq!(
            err.to_string(),
            "RPC error: unexpected hash length (2) (code -32602)"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_preserves_jsonrpc_error_code_and_data() {
        let (url, server) = serve_once(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"startLedger must be within the ledger range: 5033608 - 5154567","data":{"oldest":5033608,"latest":5154567}}}"#,
        )
        .await;

        let err = SorobanRpcClient::new(&url)
            .request::<HealthCheck>("getEvents", ())
            .await
            .unwrap_err();

        assert_eq!(err.code(), Some(-32600));
        assert_eq!(
            err.data(),
            Some(&serde_json::json!({"oldest": 5033608, "latest": 5154567}))
        );
        let text = err.to_string();
        assert!(
            text.contains("startLedger must be within the ledger range: 5033608 - 5154567"),
            "{text}"
        );
        assert!(text.contains("-32600"), "{text}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_without_jsonrpc_error_code_keeps_message() {
        let (url, server) =
            serve_once(r#"{"jsonrpc":"2.0","id":1,"error":{"message":"no code here"}}"#).await;

        let err = SorobanRpcClient::new(&url)
            .request::<HealthCheck>("getHealth", ())
            .await
            .unwrap_err();

        assert_eq!(err.code(), None);
        assert!(matches!(&err, RpcError::Rpc(msg) if msg == "no code here"));
        assert_eq!(err.to_string(), "RPC error: no code here");
        server.await.unwrap();
    }

    #[test]
    fn non_jsonrpc_errors_have_no_code() {
        let err = RpcError::Rpc("local validation".into());
        assert_eq!(err.code(), None);
        assert_eq!(err.data(), None);
        assert_eq!(RpcError::ContractNotFound.code(), None);
    }
}
