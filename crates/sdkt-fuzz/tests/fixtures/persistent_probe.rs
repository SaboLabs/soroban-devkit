#![no_std]
use soroban_sdk::{contract, contractimpl, symbol_short, Env};

#[contract]
pub struct PersistentProbe;

#[contractimpl]
impl PersistentProbe {
    pub fn peek(env: Env) -> u32 {
        env.storage().persistent().get(&symbol_short!("COUNTER")).unwrap_or(0)
    }
}
