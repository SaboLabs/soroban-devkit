//! Deterministic fixture contract for the Testnet deployment smoke workflow.
//!
//! Replaces the scaffolded `sdkt init` `src/lib.rs` during the smoke run.
//! Deliberately minimal: one state-changing entrypoint (`increment`) that
//! bumps instance storage and emits an event, plus one read-only entrypoint
//! (`get`) used for post-deployment verification. No constructor args, no
//! admin/auth surface, no production credentials. This file is never
//! deployed to Mainnet; the smoke workflow pins the SDF Testnet profile and
//! the CLI's mainnet-safety guard refuses implicit mainnet regardless.
//!
//! Identical to the counter in docs/getting-started/testnet-walkthrough.md
//! (Step 4), so the documented walkthrough and the CI smoke path validate the
//! same contract.

#![no_std]
use soroban_sdk::{contract, contractimpl, symbol_short, Env};

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    /// Bump a counter stored in instance storage and emit an `inc` event.
    ///
    /// `Events::publish` is superseded by the `#[contractevent]` macro in
    /// soroban-sdk 27; the raw form is kept here so the fixture's event topic
    /// stays a plain symbol and the smoke read-back stays deterministic.
    #[allow(deprecated)]
    pub fn increment(env: Env) -> u32 {
        let key = symbol_short!("COUNT");
        let count: u32 = env.storage().instance().get(&key).unwrap_or(0) + 1;
        env.storage().instance().set(&key, &count);
        env.events().publish((symbol_short!("inc"),), count);
        count
    }

    /// Read the current counter (read-only).
    pub fn get(env: Env) -> u32 {
        let key = symbol_short!("COUNT");
        env.storage().instance().get(&key).unwrap_or(0)
    }
}
