//! Command modules for `sdkt-cli`.
//!
//! Each module owns the orchestration for one CLI command area; `main.rs`
//! keeps the clap definitions and delegates to these modules.

pub mod abi;
pub mod network;
