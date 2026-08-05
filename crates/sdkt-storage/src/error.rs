use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("RPC error: {0}")]
    Rpc(#[from] sdkt_rpc::error::RpcError),
    #[error("Contract ID not found")]
    ContractNotFound,
    #[error("Invalid contract ID: {0}")]
    InvalidContractId(String),
    #[error("Parsing error: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Corrupt cache data: {0}")]
    CorruptCache(String),
}
