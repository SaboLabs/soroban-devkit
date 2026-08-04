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
pub mod storage;
pub mod transaction;

pub use account::{inspect_account, AccountBalance, AccountInspection, AccountSigner};
pub use client::SorobanRpcClient;
pub use error::RpcError;
pub use events::{get_contract_events, ContractEvent};
pub use fee::{estimate_dynamic_fee, get_fee_stats, FeeDistribution, FeeStats};
pub use inspect::{inspect_contract, ContractInspection, StorageKeyInfo, TtlInfoSummary};
pub use storage::{calculate_extension_cost, get_ttl_info, TtlEntry, TtlInfo};
pub use transaction::{inspect_transaction, TransactionInspection};
