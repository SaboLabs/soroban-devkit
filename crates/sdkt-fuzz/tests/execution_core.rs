//! Integration tests for the Phase 1 execution core.
//!
//! Acceptance criteria covered:
//!
//! 1. fresh-Host isolation — a case that mutates state does not affect the
//!    next case executed from the same baseline.
//! 2. baseline state reset — repeated executions of the same case observe
//!    identical state changes.
//! 3. ModuleCache reuse — the shared cache survives repeated executions
//!    (public-API check only) and keeps producing identical observations.
//! 4. observation success + error — both outcomes produce an Observation.
//! 5. determinism — same wasm + config + case ⇒ identical observations.
//! 6. error boundary — invalid setups are `FuzzError`, never Observations.
//!
//! Fixture: the repository's committed `us_new.wasm`, a real compiled
//! Soroban contract exporting `hello()` and `increment()` (the latter bumps
//! an instance-storage counter). Reused, not added.

use sdkt_fuzz::{
    ExecutionStatus, Executor, FunctionCall, FuzzCase, FuzzConfig, FuzzError, Observation,
    StateChange,
};

use soroban_env_host::xdr::{
    ContractDataDurability, ContractDataEntry, ExtensionPoint, LedgerEntry, LedgerEntryData,
    LedgerEntryExt, ScSymbol, ScVal, StringM,
};

const WASM: &[u8] = include_bytes!("../../../crates/sdkt-cli/tests/fixtures/us_new.wasm");

fn executor() -> Executor {
    Executor::new(WASM, FuzzConfig::default()).expect("executor builds against fixture wasm")
}

fn call(exec: &Executor, id: &str, function: &str, baseline: Vec<LedgerEntry>) -> FuzzCase {
    exec.case(id, FunctionCall::new(function, vec![]), baseline)
}

/// Execution content only: `case_id` is traceability metadata and is
/// deliberately excluded from cross-case comparison.
fn core(obs: &Observation) -> Observation {
    let mut o = obs.clone();
    o.case_id.clear();
    o
}

// ---------------------------------------------------------------------------
// 4. Observation — success and error
// ---------------------------------------------------------------------------

#[test]
fn observation_success_and_error() {
    let exec = executor();

    let ok = exec
        .execute(&call(&exec, "ok", "hello", vec![]))
        .expect("a successful call must produce an Observation");
    assert!(ok.is_success(), "hello() must not be a failure: {ok:?}");
    assert_eq!(ok.function, "hello");
    assert_eq!(ok.case_id, "ok");
    assert!(ok.return_value().is_some(), "hello() returns a value");

    // A missing function is a contract/host error *result*, not a FuzzError:
    // classifying it (expected vs unexpected) is an oracle concern of a
    // later phase — this core only observes.
    let bad = exec
        .execute(&call(&exec, "bad", "does_not_exist", vec![]))
        .expect("a failing call must still produce an Observation");
    assert!(!bad.is_success());
    match &bad.status {
        ExecutionStatus::ContractError { error_type, code } => {
            assert_eq!(error_type, "WasmVm", "expected a WasmVm error type");
            assert_eq!(*code, 3, "expected MissingValue (ScErrorCode = 3)");
        }
        other => panic!("expected ContractError, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 5. Determinism
// ---------------------------------------------------------------------------

#[test]
fn deterministic_observation() {
    let exec = executor();
    let a = exec.execute(&call(&exec, "det", "hello", vec![])).unwrap();
    let b = exec.execute(&call(&exec, "det", "hello", vec![])).unwrap();
    assert_eq!(a, b, "same input must produce identical observations");

    // Separately-constructed executors over the same wasm/config agree too.
    let other = executor();
    let c = other
        .execute(&call(&other, "det", "hello", vec![]))
        .unwrap();
    assert_eq!(a, c, "observations must not depend on executor identity");
}

// ---------------------------------------------------------------------------
// 3. ModuleCache reuse (public API only)
// ---------------------------------------------------------------------------

#[test]
fn module_cache_reuse() {
    let exec = executor();
    assert!(
        exec.module_cached().expect("cache query works"),
        "campaign wasm must be present in the shared module cache after construction"
    );

    let mut observations: Vec<Observation> = Vec::new();
    for i in 0..8 {
        observations.push(
            exec.execute(&call(&exec, &format!("cache-{i}"), "hello", vec![]))
                .unwrap(),
        );
        assert!(
            exec.module_cached().expect("cache query works"),
            "cache must retain the module after {i} executions"
        );
    }
    assert!(
        observations.windows(2).all(|w| core(&w[0]) == core(&w[1])),
        "every execution through the reused cache must be identical"
    );
    // The host keys the cache by this hash; it must be stable.
    assert_eq!(exec.wasm_hash(), executor().wasm_hash());
}

// ---------------------------------------------------------------------------
// 1 + 2. Fresh-Host isolation + baseline reset
// ---------------------------------------------------------------------------

#[test]
fn fresh_host_isolation_and_baseline_reset() {
    let exec = executor();

    // `increment()` mutates instance storage. If the Host or its state were
    // shared between cases, each run would see the previous counter and
    // return a strictly increasing value.
    let mut mutated: Vec<Observation> = Vec::new();
    for i in 0..3 {
        let obs = exec
            .execute(&call(&exec, &format!("mutate-{i}"), "increment", vec![]))
            .expect("increment executes");
        assert!(obs.is_success(), "increment must succeed: {obs:?}");
        assert!(
            obs.changes().count() > 0,
            "increment must record state changes in the footprint diff"
        );
        mutated.push(obs);
    }
    assert!(
        mutated.windows(2).all(|w| core(&w[0]) == core(&w[1])),
        "each increment must observe the same fresh baseline (no state leak)"
    );

    // A read-only case after the mutations sees baseline state, not them.
    let read = exec
        .execute(&call(&exec, "after", "hello", vec![]))
        .unwrap();
    for change in read.changes() {
        assert_eq!(
            change.change,
            StateChange::Unchanged,
            "a read-only case must not see previous mutations"
        );
    }
}

// ---------------------------------------------------------------------------
// Baseline entries are seeded and observable
// ---------------------------------------------------------------------------

#[test]
fn baseline_entries_are_seeded_into_every_case() {
    let exec = executor();
    let seed = seed_entry(&exec, "SEED", 41);
    let seed_key_xdr = xdr_of(&seed.to_key());
    let seed_value_xdr = xdr_of(&seed);

    let mut obs_seed: Vec<Observation> = Vec::new();
    for i in 0..3 {
        let obs = exec
            .execute(&call(
                &exec,
                &format!("seeded-{i}"),
                "hello",
                vec![seed.clone()],
            ))
            .expect("seeded execution");
        let seeded = obs
            .state
            .iter()
            .find(|s| s.key_xdr == seed_key_xdr)
            .expect("seeded entry appears in the footprint observation");
        assert_eq!(seeded.change, StateChange::Unchanged);
        assert_eq!(
            seeded.value_xdr.as_deref(),
            Some(seed_value_xdr.as_slice()),
            "the seeded value must be what the execution saw"
        );
        obs_seed.push(obs);
    }
    assert!(
        obs_seed.windows(2).all(|w| core(&w[0]) == core(&w[1])),
        "the same seed must yield the same observation in every case"
    );
}

/// Build a contract-data baseline entry for this campaign's contract.
fn seed_entry(exec: &Executor, key: &str, value: u32) -> LedgerEntry {
    let contract = exec
        .case("seed", FunctionCall::new("hello", vec![]), vec![])
        .contract_address();
    LedgerEntry {
        last_modified_ledger_seq: 0,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract,
            key: ScVal::Symbol(ScSymbol(StringM::try_from(key).unwrap())),
            durability: ContractDataDurability::Persistent,
            val: ScVal::U32(value),
        }),
        ext: LedgerEntryExt::V0,
    }
}

fn xdr_of<T: soroban_env_host::xdr::WriteXdr>(value: &T) -> Vec<u8> {
    value.to_xdr(soroban_env_host::xdr::Limits::none()).unwrap()
}

// ---------------------------------------------------------------------------
// 6. Error boundary — FuzzError, never an Observation, never a "finding"
// ---------------------------------------------------------------------------

#[test]
fn setup_and_usage_errors_are_not_observations() {
    // Empty WASM: invalid setup.
    let err = Executor::new(&[], FuzzConfig::default()).unwrap_err();
    assert!(
        matches!(err, FuzzError::InvalidSetup(_)),
        "empty wasm must be InvalidSetup: {err:?}"
    );

    // Garbage bytes: rejected by the Soroban engine while building the cache.
    let err = Executor::new(b"not a wasm module", FuzzConfig::default()).unwrap_err();
    assert!(
        matches!(err, FuzzError::InvalidSetup(_)),
        "garbage wasm must be InvalidSetup: {err:?}"
    );

    // Baseline entry belonging to another contract: invalid setup.
    let exec = executor();
    let mut foreign = seed_entry(&exec, "SEED", 1);
    if let LedgerEntryData::ContractData(d) = &mut foreign.data {
        d.contract = soroban_env_host::xdr::ScAddress::Contract(soroban_env_host::xdr::ContractId(
            soroban_env_host::xdr::Hash([9u8; 32]),
        ));
    }
    let err = exec
        .execute(&call(&exec, "foreign", "hello", vec![foreign]))
        .unwrap_err();
    assert!(
        matches!(err, FuzzError::InvalidSetup(_)),
        "foreign baseline must be InvalidSetup: {err:?}"
    );

    // The instance entry is executor-owned; supplying one is invalid setup.
    let mut instance = seed_entry(&exec, "SEED", 5);
    if let LedgerEntryData::ContractData(d) = &mut instance.data {
        d.key = ScVal::LedgerKeyContractInstance;
    }
    let err = exec
        .execute(&call(&exec, "instance-supplied", "hello", vec![instance]))
        .unwrap_err();
    assert!(
        matches!(err, FuzzError::InvalidSetup(_)),
        "caller-supplied instance entry must be InvalidSetup: {err:?}"
    );

    // Baseline entries of unsupported ledger types: invalid setup.
    let code = LedgerEntry {
        last_modified_ledger_seq: 0,
        data: LedgerEntryData::ContractCode(soroban_env_host::xdr::ContractCodeEntry {
            ext: soroban_env_host::xdr::ContractCodeEntryExt::V0,
            hash: soroban_env_host::xdr::Hash([7u8; 32]),
            code: vec![0u8; 8].try_into().unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    };
    let err = exec
        .execute(&call(&exec, "wrong-kind", "hello", vec![code]))
        .unwrap_err();
    assert!(
        matches!(err, FuzzError::InvalidSetup(_)),
        "non-ContractData baseline must be InvalidSetup: {err:?}"
    );
}

#[test]
fn invalid_configuration_is_rejected() {
    // The default config validates.
    executor();

    // Ledger sequence too close to u32::MAX for the baseline TTL window.
    let mut cfg = FuzzConfig::default();
    cfg.ledger.sequence_number = u32::MAX - 1;
    let err = Executor::new(WASM, cfg).unwrap_err();
    assert!(
        matches!(err, FuzzError::InvalidConfig(_)),
        "ledger overflow must be InvalidConfig: {err:?}"
    );

    // Custom budget limits are not supported in Phase 1.
    let mut cfg = FuzzConfig::default();
    cfg.budget.cpu_limit = Some(1_000_000);
    let err = Executor::new(WASM, cfg).unwrap_err();
    assert!(
        matches!(err, FuzzError::InvalidConfig(_)),
        "custom cpu limit must be InvalidConfig in Phase 1: {err:?}"
    );
}
