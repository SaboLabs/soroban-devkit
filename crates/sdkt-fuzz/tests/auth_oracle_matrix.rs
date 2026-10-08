//! Oracle-level AUTHORIZATION_BYPASS matrix (follow-up fix).
//!
//! Before the fix, `Oracle` had no `AuthMode` input and classified
//! `auth_required && success` as AUTHORIZATION_BYPASS unconditionally — so a
//! *correctly authorized* success was reported as a bypass (false finding).
//!
//! Corrected matrix (fixture `set` is unguarded and always succeeds, so a
//! declared `auth_required` rule on it is exactly the bypass-vs-authorized
//! distinction under test):
//!
//! | auth_required | AuthMode    | outcome | expected                    |
//! |---------------|-------------|---------|-----------------------------|
//! | false         | NoAuth      | success | not a bypass                |
//! | true          | NoAuth      | success | AUTHORIZATION_BYPASS        |
//! | true          | WrongAuth   | success | AUTHORIZATION_BYPASS        |
//! | true          | CorrectAuth | success | NOT a bypass (authorized)   |
//!
//! Requirement 5: an authorized success that matches the declaration is PASS.
//! Requirement 6: no observation-only heuristic is used — the mode is carried
//! explicitly into classification.

use sdkt_fuzz::{
    AuthMode, Classification, Environment, Expected, ExpectedBehavior, FunctionCall, Oracle,
    ReasonCode,
};
use soroban_env_host::xdr::ScVal;

const AUTH_WASM: &[u8] = include_bytes!("fixtures/auth_probe.wasm");

fn auth_required_rule() -> Expected {
    Expected {
        auth_required: true,
        behavior: ExpectedBehavior::Success {
            expect_return: None,
        },
        ..Default::default()
    }
}

fn is_bypass(c: &Classification) -> bool {
    *c == Classification::Finding(ReasonCode::AuthorizationBypass)
}

/// Execute the unguarded `set(7)` under `mode` and classify with that mode.
fn classify_set(exec: &sdkt_fuzz::Executor, mode: AuthMode) -> Classification {
    let env = Environment::default();
    let call = FunctionCall::new("set", vec![ScVal::U32(7)]);
    let case = exec.case("matrix", call.clone(), Vec::new());
    let entries = sdkt_fuzz::auth::invoke_auth_entries(mode, &case.contract_address(), &call)
        .expect("auth entries constructible for every mode");
    let obs = exec
        .execute_with_auth(&case, &env, &entries)
        .expect("set succeeds under every mode");
    assert!(obs.is_success(), "fixture precondition: set(7) succeeds");
    Oracle::with_auth_mode(auth_required_rule(), mode).classify(&obs)
}

#[test]
fn auth_required_false_noauth_success_is_not_bypass() {
    let exec = sdkt_fuzz::Executor::new(AUTH_WASM, Default::default()).unwrap();
    let env = Environment::default();
    let call = FunctionCall::new("set", vec![ScVal::U32(7)]);
    let obs = exec
        .execute_with_auth(&exec.case("m", call, Vec::new()), &env, &[])
        .unwrap();
    let oracle = Oracle::with_auth_mode(Expected::default(), AuthMode::NoAuth);
    let got = oracle.classify(&obs);
    assert!(
        !is_bypass(&got),
        "undeclared auth_required never yields a bypass; got {got:?}"
    );
}

#[test]
fn auth_required_true_noauth_success_is_bypass() {
    let exec = sdkt_fuzz::Executor::new(AUTH_WASM, Default::default()).unwrap();
    assert_eq!(
        classify_set(&exec, AuthMode::NoAuth),
        Classification::Finding(ReasonCode::AuthorizationBypass),
    );
}

#[test]
fn auth_required_true_wrongauth_success_is_bypass() {
    let exec = sdkt_fuzz::Executor::new(AUTH_WASM, Default::default()).unwrap();
    assert_eq!(
        classify_set(&exec, AuthMode::WrongAuth),
        Classification::Finding(ReasonCode::AuthorizationBypass),
    );
}

#[test]
fn auth_required_true_correctauth_success_is_not_bypass() {
    let exec = sdkt_fuzz::Executor::new(AUTH_WASM, Default::default()).unwrap();
    let got = classify_set(&exec, AuthMode::CorrectAuth);
    assert!(
        !is_bypass(&got),
        "an authorized success is not a bypass; got {got:?}"
    );
    // Requirement 5: authorized success matching the declaration is PASS.
    assert_eq!(got, Classification::Pass);
}

#[test]
fn other_rules_unaffected_by_auth_mode() {
    let exec = sdkt_fuzz::Executor::new(AUTH_WASM, Default::default()).unwrap();
    let env = Environment::default();
    let call = FunctionCall::new("set", vec![ScVal::U32(7)]);
    let obs = exec
        .execute_with_auth(&exec.case("m2", call, Vec::new()), &env, &[])
        .unwrap();

    // RETURN_MISMATCH must fire identically in every mode.
    let wrong_return = Expected {
        behavior: ExpectedBehavior::Success {
            expect_return: Some(ScVal::U32(8)),
        },
        ..Default::default()
    };
    for mode in [AuthMode::NoAuth, AuthMode::CorrectAuth, AuthMode::WrongAuth] {
        assert_eq!(
            Oracle::with_auth_mode(wrong_return.clone(), mode).classify(&obs),
            Classification::Finding(ReasonCode::ReturnMismatch),
            "RETURN_MISMATCH is auth-mode independent (mode={mode:?})"
        );
    }
}
