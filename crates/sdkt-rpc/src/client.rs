//! Soroban RPC HTTP client.
//!
//! Low-level JSON‑RPC over HTTP via [`reqwest`]. Provides only the scaffold
//! in this phase — RPC method implementations arrive in Phase 2.

use sdkt_core::NetworkConfig;
use serde::Deserialize;

/// Soroban RPC HTTP client.
///
/// Holds the RPC endpoint URL. A reusable HTTP client is added in Phase 2
/// when RPC methods are implemented.
pub struct SorobanRpcClient {
    /// Base URL of the Soroban RPC endpoint (e.g. `https://soroban-testnet.stellar.org`).
    endpoint: String,
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
}

/// Health check response from the node.
#[derive(Debug, Deserialize)]
pub struct HealthCheck {
    /// Status string (e.g. `"ok"`, `"error"`).
    pub status: String,
}

/// Ledger info snapshot.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerInfo {
    /// Current ledger sequence number.
    pub ledger: u32,
}

/// Storage response payload (placeholder for Phase 2).
#[derive(Debug, Deserialize)]
pub struct StorageResponse {
    // TODO: define Phase 2
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
