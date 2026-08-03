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

pub use client::SorobanRpcClient;
pub use error::RpcError;
