//! Minimal fixture contract for compatibility CI ABI coverage.
//!
//! Declares exactly the ABI surface the M43/M44 offline checks need to prove:
//!   - a `mint` function (functions_contain)
//!   - a `Circle` custom type (custom_types_contain)
//!   - a `Mint` event (events_contain)
//!
//! This contract is never deployed to any network. It exists solely so that
//! `sdkt wasm inspect --format json` on the compiled WASM returns a spec with
//! all three names, letting both offline steps in `compatibility.yml` make
//! hard assertions instead of WARN-only greps.
//!
//! Build:
//!   cd crates/sdkt-cli/tests/fixtures/mint_fixture
//!   cargo build --target wasm32v1-none --release
//!   cp target/wasm32v1-none/release/mint_fixture.wasm ../mint_fixture.wasm

#![no_std]
use soroban_sdk::{contract, contractevent, contractimpl, contracttype, Address, Env};

/// A simple geometric shape stored in contract state.
#[contracttype]
#[derive(Clone)]
pub struct Circle {
    /// Centre x-coordinate (fixed-point, 6 decimal places).
    pub x: i64,
    /// Centre y-coordinate (fixed-point, 6 decimal places).
    pub y: i64,
    /// Radius (fixed-point, 6 decimal places).
    pub radius: i64,
}

/// Emitted when a mint operation is performed.
#[contractevent]
pub struct Mint {
    /// The recipient of the minted amount.
    #[topic]
    pub to: Address,
    /// Amount minted.
    pub amount: i128,
}

#[contract]
pub struct MintFixture;

#[contractimpl]
impl MintFixture {
    /// Mint `amount` tokens to `to` and emit a `Mint` event.
    ///
    /// This function exists only to give the ABI a `mint` function entry and
    /// to publish the `Mint` event so both appear in the compiled spec section.
    pub fn mint(env: Env, to: Address, amount: i128) {
        Mint {
            to: to.clone(),
            amount,
        }
        .publish(&env);
    }

    /// Store a circle in instance storage (exercises the `Circle` custom type).
    pub fn set_circle(env: Env, circle: Circle) {
        env.storage()
            .instance()
            .set(&soroban_sdk::symbol_short!("circle"), &circle);
    }

    /// Retrieve the stored circle.
    pub fn get_circle(env: Env) -> Option<Circle> {
        env.storage()
            .instance()
            .get(&soroban_sdk::symbol_short!("circle"))
    }
}
