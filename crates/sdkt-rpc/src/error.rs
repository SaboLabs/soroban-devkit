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
    /// A `getLedgerEntries` request was refused before it was sent because it
    /// carried more keys than the protocol accepts in one request.
    #[error(
        "getLedgerEntries accepts at most {max} ledger keys per request (protocol maximum), \
         got {count}; split the keys into batches of {max} or fewer"
    )]
    TooManyLedgerKeys {
        /// Number of keys the caller passed.
        count: usize,
        /// Protocol maximum ([`crate::MAX_LEDGER_ENTRY_KEYS`]).
        max: usize,
    },
}
