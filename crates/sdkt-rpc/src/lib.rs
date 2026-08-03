//! # sdkt-rpc
//!
//! Soroban RPC client for the Soroban DevKit.
//!
//! Provides HTTP + JSON-RPC interactions with Soroban RPC nodes.
//!
//! ## Public API
//! - [`SorobanRpcClient`] — main client
//! - [`RpcError`] — structured error types

pub mod client;
pub mod error;
pub mod inspect;
pub mod storage;

pub use client::SorobanRpcClient;
pub use error::RpcError;
pub use inspect::{inspect_contract, ContractInspection, StorageKeyInfo};
pub use storage::{calculate_extension_cost, get_ttl_info, TtlEntry, TtlInfo};
