//! Minimal example Soroban contract used by the `sdkt` onboarding smoke test.
//!
//! It is intentionally small and self-contained. `admin_action` is a privileged
//! function that deliberately omits `require_auth()` so the `sdkt audit` static
//! analyzer produces a deterministic AUTH-001 finding (demonstrating the tool,
//! not recommending the pattern). `transfer` shows the correct guard.

use soroban_sdk::{contract, contractimpl, contracttype, Address, Symbol};

#[contracttype]
pub struct Admin {}

#[contract]
pub struct SampleToken;

#[contractimpl]
impl SampleToken {
    pub fn transfer(_from: Address, _to: Address, _amount: i128) {
        // Correctly guarded privileged entrypoint.
        _from.require_auth();
    }

    /// Privileged admin action — intentionally missing `require_auth()` so that
    /// `sdkt audit` flags it with AUTH-001. This is the demonstration target.
    pub fn admin_action(_admin: Address) {
        // NOTE: no require_auth() — sdkt audit will flag this as AUTH-001.
        let _ = Symbol::short("noop");
    }
}
