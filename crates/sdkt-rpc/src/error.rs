//! RPC-specific error types.
//!
//! [`RpcError`] aggregates transport, serialization, protocol, and contract
//! errors into a single `thiserror` enum for ergonomic `?` propagation.

use thiserror::Error;

/// Errors that can occur during Soroban RPC interaction.
#[derive(Debug, Error)]
pub enum RpcError {
    /// HTTP transport failure (network, timeout, DNS, etc.)
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON serialization/deserialization failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Raw JSON-RPC error message from the server.
    #[error("RPC error: {0}")]
    Rpc(String),

    /// Contract not found at the given address.
    #[error("contract not found")]
    ContractNotFound,
}

/// Result alias for RPC operations.
pub type Result<T> = std::result::Result<T, RpcError>;
