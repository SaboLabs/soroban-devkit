#![no_std]
use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};

#[contract]
pub struct AuthProbe;

#[contractimpl]
impl AuthProbe {
    /// Requires the caller's auth. This contract is an *auth-behavior*
    /// fixture (correct/incorrect authorization must be observable), not a
    /// vulnerability fixture.
    pub fn bump(env: Env, who: Address) -> u32 {
        who.require_auth();
        let key = symbol_short!("N");
        let n: u32 = env.storage().instance().get(&key).unwrap_or(0) + 1;
        env.storage().instance().set(&key, &n);
        n
    }

    /// No auth required; returns the argument (typed-arg exercise).
    pub fn set(n: u32) -> u32 {
        n
    }

    /// No auth required; reads state.
    pub fn peek(env: Env) -> u32 {
        env.storage().instance().get(&symbol_short!("N")).unwrap_or(0)
    }
}
