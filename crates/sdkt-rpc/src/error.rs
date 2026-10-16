use thiserror::Error;

/// Core RPC error types for Soroban DevKit.
#[derive(Debug, Error)]
pub enum RpcError {
    /// An error originating from the JSON-RPC response (e.g. invalid arguments, internal error).
    #[error("RPC error: {0}")]
    Rpc(String),
    /// A JSON-RPC error object returned by the node, with its machine-readable
    /// `code` (e.g. `-32602` invalid params, `-32600` invalid request) and the
    /// optional `data` member, so callers can branch on the failure cause
    /// without parsing `message`.
    #[error("RPC error: {message} (code {code})")]
    JsonRpc {
        /// JSON-RPC error code.
        code: i64,
        /// Human-readable message from the node, verbatim.
        message: String,
        /// Optional structured detail from the node, verbatim.
        data: Option<serde_json::Value>,
    },
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

impl RpcError {
    /// The JSON-RPC error code, when this error came from a node's JSON-RPC
    /// error object.
    pub fn code(&self) -> Option<i64> {
        match self {
            RpcError::JsonRpc { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// The JSON-RPC error `data` member, when the node sent one.
    pub fn data(&self) -> Option<&serde_json::Value> {
        match self {
            RpcError::JsonRpc { data, .. } => data.as_ref(),
            _ => None,
        }
    }
}
