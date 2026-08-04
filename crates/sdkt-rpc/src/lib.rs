//! RPC module handles network requests.
//!
//! Exposes clients and methods to query the Soroban RPC endpoint for contract
//! inspection, XDR retrieval, and storage proofs.
//!
//! # Modules
//!
//! - [`SorobanRpcClient`] — main client
//! - [`RpcError`] — structured error types

pub mod account;
pub mod client;
pub mod error;
pub mod events;
pub mod fee;
pub mod inspect;
pub mod simulate;
pub mod storage;
pub mod submission;
pub mod transaction;
pub mod wasm;

pub use account::{inspect_account, AccountBalance, AccountInspection, AccountSigner};
pub use client::SorobanRpcClient;
pub use error::RpcError;
pub use events::{get_contract_events, ContractEvent};
pub use fee::{estimate_dynamic_fee, get_fee_stats, FeeDistribution, FeeStats};
pub use inspect::{inspect_contract, ContractInspection, StorageKeyInfo, TtlInfoSummary};
pub use simulate::{
    simulate_transaction, validate_envelope, SimulateCost, SimulateOperationResult,
    SimulateResponse, SimulateTransactionRequest,
};
pub use storage::{calculate_extension_cost, get_ttl_info, TtlEntry, TtlInfo};
pub use submission::{
    get_transaction_status, poll_transaction, send_transaction, submit_and_wait, PollConfig,
    SendTransactionRequest, SendTransactionResponse, SubmissionResult, TransactionStatus,
    TransactionStatusResponse,
};
pub use transaction::{inspect_transaction, TransactionInspection};
pub use wasm::get_wasm_metadata;
