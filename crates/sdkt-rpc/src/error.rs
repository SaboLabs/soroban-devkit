use thiserror::Error;

/// Errors from Soroban RPC operations.
#[derive(Error, Debug)]
pub enum RpcError {
    /// HTTP transport failure.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON serialization/deserialization failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// JSON-RPC level error returned by the server.
    #[error("RPC error: {0}")]
    Rpc(String),

    /// Invalid request parameters (client-side guard).
    #[error("invalid params: {0}")]
    InvalidParams(String),

    /// The requested contract was not found on-chain.
    #[error("contract not found")]
    ContractNotFound,

    /// The requested ledger entry was not found.
    #[error("entry not found")]
    EntryNotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_error_display() {
        let e = RpcError::Rpc("timeout".to_string());
        assert_eq!(e.to_string(), "RPC error: timeout");
        let e = RpcError::ContractNotFound;
        assert_eq!(e.to_string(), "contract not found");
        let e = RpcError::InvalidParams("empty key".to_string());
        assert_eq!(e.to_string(), "invalid params: empty key");
    }
}
