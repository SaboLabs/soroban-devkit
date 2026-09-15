use thiserror::Error;

/// Core RPC error types for Soroban DevKit.
#[derive(Debug, Error)]
pub enum RpcError {
    /// An error originating from the JSON-RPC response (e.g. invalid arguments, internal error).
    #[error("RPC error: {0}")]
    Rpc(String),
    /// An error related to network connectivity or HTTP protocol.
    #[error("HTTP request error: {0}")]
    Reqwest(#[from] reqwest::Error),
    /// An error parsing or structuring JSON data.
    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),
    /// Configuration or environment error.
    #[error("Configuration error: {0}")]
    Config(String),
    /// Contract was not found on the network.
    #[error("Contract not found on the network")]
    ContractNotFound,
}
