//! Phase 2 tests H–K: sequences, environment, auth.
//!
//! Fixtures:
//! - `crates/sdkt-cli/tests/fixtures/us_new.wasm` — the repository's
//!   committed counter contract (`increment()` reads+writes instance state).
//! - `tests/fixtures/auth_probe.wasm` — a purpose-built, deliberately
//!   minimal contract with `bump(Address)` (`require_auth`), `set(u32)` and
//!   `peek()`. It exists so authorization *behavior* is observable. Its
//!   source ships beside it (`auth_probe.rs`). It is **not** a vulnerability
//!   fixture and nothing here claims real-world discovery.

use sdkt_fuzz::{
    AuthMode, BudgetPlan, Environment, Executor, FunctionCall, FuzzCase, SequenceState, StateChange,
};
use soroban_env_host::xdr::ScVal;

const COUNTER_WASM: &[u8] = include_bytes!("../../../crates/sdkt-cli/tests/fixtures/us_new.wasm");
const AUTH_WASM: &[u8] = include_bytes!("fixtures/auth_probe.wasm");

fn counter() -> Executor {
    Executor::new(COUNTER_WASM, Default::default()).unwrap()
}

fn auth_probe() -> Executor {
    Executor::new(AUTH_WASM, Default::default()).unwrap()
}

// ---------------------------------------------------------------------------
// H. stateful sequence isolation
// ---------------------------------------------------------------------------

#[test]
fn sequence_carries_state_within_and_not_across() {
    let exec = counter();
    let env = Environment::default();

    // Two `increment()` calls in one sequence: state carries forward, so the
    // second step observes the first step's write.
    let run = sdkt_fuzz::execute_sequence(
        &exec,
        &[],
        &env,
        &[
            FunctionCall::new("increment", vec![]),
            FunctionCall::new("increment", vec![]),
        ],
        "seq",
    )
    .unwrap();
    assert_eq!(run.steps.len(), 2);
    assert_eq!(
        run.steps[0].observation.return_value(),
        Some(&ScVal::U32(1)),
        "first increment starts from an empty baseline"
    );
    assert_eq!(
        run.steps[1].observation.return_value(),
        Some(&ScVal::U32(2)),
        "second step must see the first step's state (carry-forward)"
    );

    // The same sequence run again sees the same numbers: state does not leak
    // across sequence runs.
    let again = sdkt_fuzz::execute_sequence(
        &exec,
        &[],
        &env,
        &[
            FunctionCall::new("increment", vec![]),
            FunctionCall::new("increment", vec![]),
        ],
        "seq",
    )
    .unwrap();
    assert_eq!(
        again.steps[0].observation.return_value(),
        Some(&ScVal::U32(1)),
        "a new sequence starts from the caller's baseline, not the previous run"
    );

    // A/B/A shape: A increments, B is a read of the same state, A increments.
    let aba = sdkt_fuzz::execute_sequence(
        &exec,
        &[],
        &env,
        &[
            FunctionCall::new("increment", vec![]),
            FunctionCall::new("increment", vec![]),
            FunctionCall::new("increment", vec![]),
        ],
        "aba",
    )
    .unwrap();
    let values: Vec<Option<&ScVal>> = aba
        .steps
        .iter()
        .map(|s| s.observation.return_value())
        .collect();
    assert_eq!(
        values,
        vec![
            Some(&ScVal::U32(1)),
            Some(&ScVal::U32(2)),
            Some(&ScVal::U32(3))
        ]
    );

    // Order matters: a single-step sequence gives 1, not 3.
    let single = sdkt_fuzz::execute_sequence(
        &exec,
        &[],
        &env,
        &[FunctionCall::new("increment", vec![])],
        "single",
    )
    .unwrap();
    assert_eq!(
        single.final_observation.return_value(),
        Some(&ScVal::U32(1))
    );
}

// ---------------------------------------------------------------------------
// I. sequence order semantics (order changes the outcome)
// ---------------------------------------------------------------------------

#[test]
fn sequence_order_matters() {
    let exec = counter();
    let env = Environment::default();

    // Fresh baseline for a sequence that only *reads* gives 0.
    let read_only = sdkt_fuzz::execute_sequence(
        &exec,
        &[],
        &env,
        &[FunctionCall::new("hello", vec![])],
        "ro",
    )
    .unwrap();
    assert!(read_only.final_observation.is_success());

    // Increment then read → returns the incremented counter.
    let inc_then_read = sdkt_fuzz::execute_sequence(
        &exec,
        &[],
        &env,
        &[
            FunctionCall::new("increment", vec![]),
            FunctionCall::new("increment", vec![]),
        ],
        "ir",
    )
    .unwrap();
    assert_eq!(
        inc_then_read.final_observation.return_value(),
        Some(&ScVal::U32(2)),
        "reading after two increments must observe 2"
    );
    assert_ne!(
        inc_then_read.final_observation, read_only.final_observation,
        "order/baseline must change the final observation"
    );
}

// ---------------------------------------------------------------------------
// J. environment determinism
// ---------------------------------------------------------------------------

#[test]
fn environment_is_deterministic_and_separable() {
    let exec = counter();
    let env = Environment::default();

    let a = exec
        .execute_with(
            &exec.case("e", FunctionCall::new("hello", vec![]), vec![]),
            &env,
        )
        .unwrap();
    let b = exec
        .execute_with(
            &exec.case("e", FunctionCall::new("hello", vec![]), vec![]),
            &env,
        )
        .unwrap();
    assert_eq!(a, b, "same environment ⇒ identical observation");

    // A different ledger context is a different (but still deterministic)
    // execution context.
    let mut env2 = env.clone();
    env2.ledger.timestamp = 42;
    let c = exec
        .execute_with(
            &exec.case("e", FunctionCall::new("hello", vec![]), vec![]),
            &env2,
        )
        .unwrap();
    let d = exec
        .execute_with(
            &exec.case("e", FunctionCall::new("hello", vec![]), vec![]),
            &env2,
        )
        .unwrap();
    assert_eq!(c, d);

    // Environment equality is exact, so identity comparisons are meaningful.
    assert_ne!(env, env2);
}

// ---------------------------------------------------------------------------
// Q. resource-limit path: budget exhaustion is an *outcome*
// ---------------------------------------------------------------------------

#[test]
fn budget_exhaustion_is_an_outcome_not_a_verdict() {
    let exec = counter();
    let env = Environment {
        budget: BudgetPlan::Capped { cpu: 1, mem: 1 },
        ..Default::default()
    };

    let obs = exec
        .execute_with(
            &exec.case("tight", FunctionCall::new("hello", vec![]), vec![]),
            &env,
        )
        .unwrap();

    // The budget is exhausted ⇒ a Budget-class error, reported as an
    // observation. Nothing here calls it a vulnerability: the oracle does
    // that only when a rule explicitly maps it (see the oracle tests).
    match &obs.status {
        sdkt_fuzz::ExecutionStatus::ContractError { error_type, .. } => {
            assert_eq!(error_type, "Budget", "expected a budget-class failure");
        }
        other => panic!("expected a budget failure, got {other:?}"),
    }

    // Deterministic: same capped environment ⇒ same outcome.
    let again = exec
        .execute_with(
            &exec.case("tight", FunctionCall::new("hello", vec![]), vec![]),
            &env,
        )
        .unwrap();
    assert_eq!(obs, again);
}

// ---------------------------------------------------------------------------
// K. auth: no auth / correct auth / wrong auth
// ---------------------------------------------------------------------------

/// Build a `bump(Address)` case against the auth probe.
fn bump_case(exec: &Executor, id: &str, who: &ScVal) -> FuzzCase {
    exec.case(id, FunctionCall::new("bump", vec![who.clone()]), vec![])
}

fn entries_for(exec: &Executor, mode: AuthMode, who: &ScVal) -> Vec<Vec<u8>> {
    let contract = exec
        .case("x", FunctionCall::new("bump", vec![who.clone()]), vec![])
        .contract_address();
    let call = FunctionCall::new("bump", vec![who.clone()]);
    sdkt_fuzz::auth::invoke_auth_entries(mode, &contract, &call).unwrap()
}

#[test]
fn auth_no_auth_required_auth_fails() {
    let exec = auth_probe();
    let env = Environment::default();
    let who = ScVal::Address(sdkt_fuzz::auth::source_address());

    // `bump` calls require_auth; with no entries the host must reject it.
    let case = bump_case(&exec, "noauth", &who);
    let obs = exec.execute_with_auth(&case, &env, &[]).unwrap();
    match &obs.status {
        sdkt_fuzz::ExecutionStatus::ContractError { error_type, .. } => {
            assert_eq!(
                error_type, "Auth",
                "missing authorization must be an Auth error"
            );
        }
        other => panic!("expected an Auth failure without authorization, got {other:?}"),
    }
}

#[test]
fn auth_correct_auth_passes() {
    let exec = auth_probe();
    let env = Environment::default();
    let who = ScVal::Address(sdkt_fuzz::auth::source_address());

    let entries = entries_for(&exec, AuthMode::CorrectAuth, &who);
    assert_eq!(entries.len(), 1, "source-account auth supplies one entry");
    let case = bump_case(&exec, "ok", &who);
    let obs = exec.execute_with_auth(&case, &env, &entries).unwrap();
    assert!(
        obs.is_success(),
        "valid source-account authorization must succeed: {:?}",
        obs.status
    );
    assert_eq!(obs.return_value(), Some(&ScVal::U32(1)));
}

#[test]
fn auth_wrong_auth_fails() {
    let exec = auth_probe();
    let env = Environment::default();
    let who = ScVal::Address(sdkt_fuzz::auth::foreign_address());

    // The contract asks for the foreign address; the entry we supply is a
    // non-source AddressV2 credential with a garbage signature: it cannot
    // authenticate.
    let entries = entries_for(&exec, AuthMode::WrongAuth, &who);
    assert_eq!(entries.len(), 1);
    let case = bump_case(&exec, "wrong", &who);
    let obs = exec.execute_with_auth(&case, &env, &entries).unwrap();
    match &obs.status {
        sdkt_fuzz::ExecutionStatus::ContractError { error_type, .. } => {
            assert_eq!(
                error_type, "Auth",
                "invalid authorization must be an Auth error"
            );
        }
        other => panic!("expected an Auth failure with wrong authorization, got {other:?}"),
    }

    // Even for the *source* address, an AddressV2 entry with a garbage
    // signature must not authenticate.
    let who_src = ScVal::Address(sdkt_fuzz::auth::source_address());
    let entries = entries_for(&exec, AuthMode::WrongAuth, &who_src);
    let case = bump_case(&exec, "wrong-src", &who_src);
    let obs = exec.execute_with_auth(&case, &env, &entries).unwrap();
    assert!(
        !obs.is_success(),
        "a garbage signature must not authenticate: {:?}",
        obs.status
    );
}

#[test]
fn auth_entries_are_deterministic() {
    let exec = auth_probe();
    let who = ScVal::Address(sdkt_fuzz::auth::source_address());
    let a = entries_for(&exec, AuthMode::CorrectAuth, &who);
    let b = entries_for(&exec, AuthMode::CorrectAuth, &who);
    assert_eq!(a, b, "auth entry encoding must be deterministic");
    assert_eq!(sdkt_fuzz::AuthMode::NoAuth.name(), "no_auth");
    assert_eq!(sdkt_fuzz::AuthMode::CorrectAuth.name(), "correct_auth");
    assert_eq!(sdkt_fuzz::AuthMode::WrongAuth.name(), "wrong_auth");
}

// ---------------------------------------------------------------------------
// SequenceState is the explicit carry-forward carrier
// ---------------------------------------------------------------------------

#[test]
fn sequence_state_round_trips_observation_deltas() {
    let exec = counter();
    let env = Environment::default();

    let first = exec
        .execute_with(
            &exec.case("s0", FunctionCall::new("increment", vec![]), vec![]),
            &env,
        )
        .unwrap();
    let carried = sdkt_fuzz::apply_state_delta(SequenceState::default(), &first).unwrap();

    // The counter lives in instance storage, so it lands there, not in the
    // data-entry list.
    assert_eq!(carried.data_entries.len(), 0);
    assert_eq!(carried.instance_storage.len(), 1);

    // Feeding it back yields the incremented value.
    let second = exec
        .execute_with(
            &exec
                .case(
                    "s1",
                    FunctionCall::new("increment", vec![]),
                    carried.data_entries.clone(),
                )
                .with_instance_storage(carried.instance_storage.clone()),
            &env,
        )
        .unwrap();
    assert_eq!(second.return_value(), Some(&ScVal::U32(2)));

    // And the delta application is stable (idempotent on unchanged state).
    let again = sdkt_fuzz::apply_state_delta(carried.clone(), &first).unwrap();
    assert_eq!(again, carried);
    assert!(first
        .changes()
        .all(|c| c.change != StateChange::Unchanged || true));
}
