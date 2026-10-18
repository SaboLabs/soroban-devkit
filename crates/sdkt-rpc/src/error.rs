use thiserror::Error;

/// Core RPC error types for Soroban DevKit.
///
/// Marked `#[non_exhaustive]`: new variants may be added in minor releases,
/// so downstream matches must keep a catch-all arm.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RpcError {
    /// The caller supplied more keys than one `getLedgerEntries` request allows.
    #[error("getLedgerEntries accepts at most {max} keys per request; received {count}. Use fetch_ledger_entries to read larger key sets in batches")]
    TooManyLedgerKeys { count: usize, max: usize },
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
    /// The RPC endpoint answered with a non-success HTTP status (e.g. `429`,
    /// `503`). Carries the numeric status so a caller can tell a server-side
    /// outage from a malformed body, and so retryable statuses can be named
    /// once the retry budget is exhausted.
    #[error("HTTP request failed with status {status}")]
    HttpStatus { status: u16 },
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
