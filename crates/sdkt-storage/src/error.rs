use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("RPC error: {0}")]
    Rpc(#[from] sdkt_rpc::error::RpcError),
    #[error("Contract ID not found")]
    ContractNotFound,
    #[error("Invalid contract ID: {0}")]
    InvalidContractId(String),
    #[error("Snapshot contract ID mismatch: old '{old}' vs new '{new}'")]
    ContractIdMismatch { old: String, new: String },
    #[error("Parsing error: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("no default identity set; choose one with `sdkt identity default <name>`")]
    NoDefaultIdentity,
    #[error("Corrupt cache data: {0}")]
    CorruptCache(String),
    #[error("Config error: {0}")]
    ConfigError(String),
}
