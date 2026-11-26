//! Phase 2 tests L–T: oracle classifications, auth bypass, minimizer,
//! campaign integration, determinism.
//!
//! Fixture honesty: the deliberately-built fixtures (`us_new.wasm` counter,
//! `tests/fixtures/auth_probe.wasm`) produce findings only because the
//! *rules in these tests declare* them. That is known fixture behavior
//! exercised through the explicit oracle — never real-world vulnerability
//! discovery.

use std::collections::BTreeMap;

use sdkt_fuzz::{
    replay, run_campaign, run_campaign_artifacts, sequence_case_counts, Classification,
    Environment, EventRecord, ExecutionStatus, Executor, Expected, ExpectedBehavior, FunctionCall,
    FuzzRunOptions, GenerationCaps, MinimizationTarget, Mutation, Operator, Oracle, ReasonCode,
    ReplayOptions, SequenceOptions,
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

/// Declared-error expectation for `bump` under NoAuth: an Auth/6 failure.
fn declared_auth_error() -> Expected {
    Expected {
        behavior: ExpectedBehavior::Error {
            error_type: "Auth".to_string(),
            code: 6,
        },
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// L. expected error oracle
// ---------------------------------------------------------------------------

#[test]
fn oracle_expected_error_matches_declared_auth_error() {
    let exec = auth_probe();
    let env = Environment::default();
    let who = ScVal::Address(sdkt_fuzz::auth::source_address());
    let call = FunctionCall::new("bump", vec![who]);
    let obs = exec
        .execute_with_auth(&exec.case("l", call, vec![]), &env, &[])
        .unwrap();

    let oracle = Oracle::new(declared_auth_error());
    assert_eq!(
        oracle.classify(&obs),
        Classification::ExpectedError,
        "declared Auth/6 must match the observed Auth failure"
    );

    // A *different* declared error must not match: the declared expectation
    // is violated (Auth/6 observed where Contract/1 was declared).
    let wrong = Oracle::new(Expected {
        behavior: ExpectedBehavior::Error {
            error_type: "Contract".to_string(),
            code: 1,
        },
        ..Default::default()
    });
    assert_eq!(
        wrong.classify(&obs),
        Classification::Finding(ReasonCode::UnexpectedError)
    );
}

// ---------------------------------------------------------------------------
// M. return mismatch / P. unexpected error
// ---------------------------------------------------------------------------

#[test]
fn oracle_return_mismatch_and_unexpected_error() {
    let exec = counter();
    let env = Environment::default();

    // M: success expected with a specific return, actual differs.
    let ok_obs = exec
        .execute_with(
            &exec.case("m", FunctionCall::new("hello", vec![]), vec![]),
            &env,
        )
        .unwrap();
    assert_eq!(ok_obs.return_value(), Some(&ScVal::U32(42)));
    let oracle = Oracle::new(Expected {
        behavior: ExpectedBehavior::Success {
            expect_return: Some(ScVal::U32(43)),
        },
        ..Default::default()
    });
    assert_eq!(
        oracle.classify(&ok_obs),
        Classification::Finding(ReasonCode::ReturnMismatch)
    );

    // Matching return passes.
    let oracle = Oracle::new(Expected {
        behavior: ExpectedBehavior::Success {
            expect_return: Some(ScVal::U32(42)),
        },
        ..Default::default()
    });
    assert_eq!(oracle.classify(&ok_obs), Classification::Pass);

    // P: Success declared, execution produced an error → UNEXPECTED_ERROR.
    let who = ScVal::Address(sdkt_fuzz::auth::source_address());
    let err_obs = exec
        .execute_with(
            &exec.case("p", FunctionCall::new("no_such_fn", vec![]), vec![]),
            &env,
        )
        .unwrap();
    assert!(matches!(
        err_obs.status,
        ExecutionStatus::ContractError { .. }
    ));
    let oracle = Oracle::new(Expected {
        behavior: ExpectedBehavior::Success {
            expect_return: None,
        },
        ..Default::default()
    });
    assert_eq!(
        oracle.classify(&err_obs),
        Classification::Finding(ReasonCode::UnexpectedError)
    );

    // ERROR ≠ vulnerability: the same error with `Any` expectation passes.
    let oracle = Oracle::new(Expected::default()); // Any
    assert_eq!(oracle.classify(&err_obs), Classification::Pass);
    let _ = who;
}

// ---------------------------------------------------------------------------
// N. state mismatch / O. event mismatch
// ---------------------------------------------------------------------------

#[test]
fn oracle_state_and_event_mismatch() {
    let exec = counter();
    let env = Environment::default();

    // N: declared state that contradicts what the contract writes.
    let obs = exec
        .execute_with(
            &exec.case("n", FunctionCall::new("increment", vec![]), vec![]),
            &env,
        )
        .unwrap();
    assert!(obs.is_success());

    // Find a changed entry, then declare a different value for it.
    let changed = obs
        .state
        .iter()
        .find(|s| s.change == sdkt_fuzz::StateChange::Updated)
        .expect("increment updates instance state");
    let mut want = BTreeMap::new();
    want.insert(changed.key_xdr.clone(), changed.value_xdr.clone().unwrap());
    // Tamper with the declared value: the key stays, the value doesn't match.
    let mut tampered = want.clone();
    tampered.insert(
        changed.key_xdr.clone(),
        changed
            .value_xdr
            .clone()
            .unwrap()
            .into_iter()
            .map(|b| b ^ 0xff)
            .collect::<Vec<u8>>(),
    );
    let oracle = Oracle::new(Expected {
        state: Some(tampered),
        ..Default::default()
    });
    assert_eq!(
        oracle.classify(&obs),
        Classification::Finding(ReasonCode::StateMismatch)
    );

    // The honest declaration matches.
    let oracle = Oracle::new(Expected {
        state: Some(want),
        ..Default::default()
    });
    assert_eq!(oracle.classify(&obs), Classification::Pass);

    // O: declared events that differ from observed events.
    // The counter fixture's `increment` emits no contract events, so a
    // declared event list is a mismatch against the empty observation.
    let declared = vec![EventRecord {
        contract_id: None,
        event_type: "Contract".to_string(),
        topics: vec![],
        data: ScVal::U32(1),
    }];
    let oracle = Oracle::new(Expected {
        events: Some(declared),
        ..Default::default()
    });
    assert_eq!(
        oracle.classify(&obs),
        Classification::Finding(ReasonCode::EventMismatch)
    );

    // Declared events equal to observed (empty) passes.
    let oracle = Oracle::new(Expected {
        events: Some(vec![]),
        ..Default::default()
    });
    assert_eq!(oracle.classify(&obs), Classification::Pass);
}

// ---------------------------------------------------------------------------
// Q. resource-limit classification (explicit rule only)
// ---------------------------------------------------------------------------

#[test]
fn oracle_resource_limit_needs_the_explicit_rule() {
    let exec = counter();
    let env = Environment {
        budget: sdkt_fuzz::BudgetPlan::Capped { cpu: 1, mem: 1 },
        ..Default::default()
    };
    let obs = exec
        .execute_with(
            &exec.case("q", FunctionCall::new("hello", vec![]), vec![]),
            &env,
        )
        .unwrap();
    assert!(matches!(
        obs.status,
        ExecutionStatus::ContractError { ref error_type, .. } if error_type == "Budget"
    ));

    // Success declared WITHOUT the rule: budget error → UNEXPECTED_ERROR,
    // not RESOURCE_LIMIT.
    let success = Expected {
        behavior: ExpectedBehavior::Success {
            expect_return: None,
        },
        ..Default::default()
    };
    let oracle = Oracle::new(success);
    assert_eq!(
        oracle.classify(&obs),
        Classification::Finding(ReasonCode::UnexpectedError)
    );

    // Success declared WITH the rule: budget error → RESOURCE_LIMIT.
    let with_rule = Expected {
        behavior: ExpectedBehavior::Success {
            expect_return: None,
        },
        resource_limit_is_finding: true,
        ..Default::default()
    };
    let oracle = Oracle::new(with_rule);
    assert_eq!(
        oracle.classify(&obs),
        Classification::Finding(ReasonCode::ResourceLimit)
    );

    // Under `Any`, even with the rule: nothing declared, nothing to violate.
    let oracle = Oracle::new(Expected {
        resource_limit_is_finding: true,
        ..Default::default()
    });
    assert_eq!(oracle.classify(&obs), Classification::Pass);
}

// ---------------------------------------------------------------------------
// K (oracle half). explicit auth-bypass finding + no-inference rule
// ---------------------------------------------------------------------------

#[test]
fn oracle_auth_bypass_finding_and_no_inference() {
    let exec = auth_probe();
    let env = Environment::default();
    let who = ScVal::Address(sdkt_fuzz::auth::source_address());

    // `peek()` needs no authorization and succeeds.
    let obs = exec
        .execute_with(
            &exec.case("k", FunctionCall::new("peek", vec![]), vec![]),
            &env,
        )
        .unwrap();
    assert!(obs.is_success());

    // Without a declared auth requirement: a success is a pass — the host's
    // enforcing mode does NOT make every success a bypass (no inference).
    let oracle = Oracle::new(Expected::default());
    assert_eq!(oracle.classify(&obs), Classification::Pass);

    // With an explicit auth requirement declared for this function, the same
    // success under the NoAuth execution becomes an AUTHORIZATION_BYPASS
    // finding. (Known fixture behavior: the rule — not the contract —
    // declares the requirement.)
    let declared = Expected {
        auth_required: true,
        behavior: ExpectedBehavior::Success {
            expect_return: None,
        },
        ..Default::default()
    };
    let oracle = Oracle::new(declared.clone());
    assert_eq!(
        oracle.classify(&obs),
        Classification::Finding(ReasonCode::AuthorizationBypass)
    );

    // Declared auth + declared error + observed error: EXPECTED_ERROR wins —
    // a *declared* failure is not a bypass (authorization was demanded and
    // the demand was met with an error, per declaration).
    let declared_error = Expected {
        auth_required: true,
        ..declared_auth_error()
    };
    let who_call = FunctionCall::new("bump", vec![who]);
    let noauth_obs = exec
        .execute_with_auth(&exec.case("k2", who_call, vec![]), &env, &[])
        .unwrap();
    let oracle = Oracle::new(declared_error);
    assert_eq!(oracle.classify(&noauth_obs), Classification::ExpectedError);
}

// ---------------------------------------------------------------------------
// R + S. minimizer preservation + determinism
// ---------------------------------------------------------------------------

/// Oracle under which `bump` under NoAuth is a *declared* auth bypass.
fn bypass_oracle() -> Oracle {
    Oracle::new(Expected {
        auth_required: true,
        behavior: ExpectedBehavior::Success {
            expect_return: None,
        },
        ..Default::default()
    })
}

#[test]
fn minimizer_is_deterministic_and_preserves_the_finding() {
    let exec = auth_probe();
    let env = Environment::default();
    // The campaign executes sequences *without* supplying auth entries
    // (auth-mode wiring into campaign execution is Phase 3 scope). So the
    // final step must be one that succeeds under NoAuth: `peek()` reads
    // instance state and needs no authorization. A declared `auth_required`
    // rule on a *successful* NoAuth execution is exactly the
    // AUTHORIZATION_BYPASS trigger (known fixture behavior: the rule
    // declares the requirement, the fixture has no require_auth — which is
    // what makes the declaration satisfiable).
    let steps = vec![
        FunctionCall::new("set", vec![ScVal::U32(99)]),
        FunctionCall::new("peek", vec![]),
    ];

    let original_obs = {
        let run = sdkt_fuzz::execute_sequence(&exec, &[], &env, &steps, "min").unwrap();
        run.final_observation
    };
    let oracle = bypass_oracle();
    let original = oracle.classify(&original_obs);
    assert_eq!(
        original,
        Classification::Finding(ReasonCode::AuthorizationBypass)
    );

    let run = |input_steps: Vec<FunctionCall>| {
        sdkt_fuzz::minimize_with_oracle(
            &exec,
            MinimizationTarget {
                steps: input_steps,
                baseline: vec![],
                environment: env.clone(),
            },
            Some(&oracle),
            &original,
            original_obs.is_success(),
        )
        .unwrap()
    };

    let a = run(steps.clone());
    let b = run(steps.clone());

    // S: determinism — same input ⇒ identical outcome and reduction trace.
    assert_eq!(a.steps, b.steps);
    assert_eq!(a.minimization, b.minimization);

    // R: preservation — still a Finding with the same reason code.
    assert_eq!(
        original,
        Classification::Finding(ReasonCode::AuthorizationBypass)
    );
    assert!(a.minimization.attempted);
    assert!(a.minimization.preserved, "reduction must keep the finding");
    assert_eq!(
        a.minimization.strategy,
        "deterministically minimized under the configured strategy"
    );

    // Reducing removable steps is legal as long as the *finding* survives —
    // which is what preservation asserts. The sequence must shrink when
    // steps are removable.
    assert!(
        a.steps.len() < steps.len(),
        "minimizer must reduce the sequence when steps are removable"
    );
    assert!(
        !a.steps.is_empty(),
        "the sequence never minimizes to zero steps"
    );
    assert!(a.minimization.minimized_complexity <= a.minimization.original_complexity);

    // A minimized run really reproduces the finding through the executor +
    // oracle (the minimizer never mutated the oracle rules).
    let final_obs = sdkt_fuzz::execute_sequence(&exec, &[], &env, &a.steps, "check")
        .unwrap()
        .final_observation;
    assert_eq!(oracle.classify(&final_obs), original);
}

#[test]
fn minimizer_reduces_argument_values_when_it_preserves() {
    let exec = auth_probe();
    let env = Environment::default();
    // `set(99)` returns 99; the rule declares 98 ⇒ RETURN_MISMATCH, and the
    // argument value stays observable to the oracle.
    let oracle = Oracle::new(Expected {
        behavior: ExpectedBehavior::Success {
            expect_return: Some(ScVal::U32(98)),
        },
        ..Default::default()
    });
    let steps = vec![FunctionCall::new("set", vec![ScVal::U32(99)])];
    let obs = sdkt_fuzz::execute_sequence(&exec, &[], &env, &steps, "arg")
        .unwrap()
        .final_observation;
    let original = oracle.classify(&obs);
    assert_eq!(
        original,
        Classification::Finding(ReasonCode::ReturnMismatch),
        "the declared return (99) differs from the actual return (99) →          declared/actual mismatch ⇒ RETURN_MISMATCH"
    );

    let outcome = sdkt_fuzz::minimize_with_oracle(
        &exec,
        MinimizationTarget {
            steps: steps.clone(),
            baseline: vec![],
            environment: env.clone(),
        },
        Some(&oracle),
        &original,
        obs.is_success(),
    )
    .unwrap();

    // Reducing set(99) → set(0) yields return 0, which still mismatches the
    // declared 98: classification and reason code unchanged, so the
    // reduction is accepted and the finding is preserved.
    assert!(outcome.minimization.preserved);
    assert_eq!(
        outcome.steps[0].args[0],
        ScVal::U32(0),
        "a finding-preserving integer reduction must be accepted"
    );
    assert!(outcome.minimization.minimized_complexity < outcome.minimization.original_complexity);

    let final_obs = sdkt_fuzz::execute_sequence(&exec, &[], &env, &outcome.steps, "check2")
        .unwrap()
        .final_observation;
    assert_eq!(oracle.classify(&final_obs), original);
    let final_obs = sdkt_fuzz::execute_sequence(&exec, &[], &env, &outcome.steps, "check2")
        .unwrap()
        .final_observation;
    assert_eq!(oracle.classify(&final_obs), original);
}

// ---------------------------------------------------------------------------
// T. campaign integration + determinism (same inputs ⇒ same outputs)
// ---------------------------------------------------------------------------

fn campaign_result(seed: [u8; 32]) -> (sdkt_fuzz::CampaignResult, Vec<String>) {
    let mut input = sdkt_fuzz::CampaignInput::new(COUNTER_WASM, seed).unwrap();
    input.cases = 4;
    input.mutations_per_case = 2;
    input.sequence_every = 0;

    // Declared rule: `hello` must return 43 → every generated `hello` case
    // violates it (known fixture behavior through an explicit declaration).
    let mut expectations = BTreeMap::new();
    expectations.insert(
        "hello".to_string(),
        Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: Some(ScVal::U32(43)),
            },
            ..Default::default()
        },
    );
    let result = run_campaign(&input, &expectations).unwrap();
    let lines: Vec<String> = result
        .findings
        .iter()
        .map(|f| {
            format!(
                "{}|{}|{}|{}",
                f.identity.case_id,
                f.identity.function,
                f.reason_code.name(),
                f.minimization.minimized_complexity
            )
        })
        .collect();
    (result, lines)
}

#[test]
fn campaign_runs_end_to_end_and_is_deterministic() {
    let (result, lines) = campaign_result([21u8; 32]);
    assert_eq!(result.executed, 4, "every case executes");
    assert!(
        result.passed.len() + result.expected_errors.len() + result.findings.len() == 4,
        "every case classifies exactly once"
    );
    assert!(
        !result.findings.is_empty(),
        "the declared hello-expectation must yield findings: {:?}",
        result
    );
    // Known fixture behavior, explicitly labeled: hello() returns 42, the
    // rule declares 43 ⇒ RETURN_MISMATCH — not a real-world vulnerability.
    for f in &result.findings {
        if f.identity.function == "hello" {
            assert_eq!(f.reason_code, ReasonCode::ReturnMismatch);
        }
        assert!(
            f.minimization.attempted,
            "findings are minimized under the configured strategy"
        );
        assert_eq!(
            f.minimization.strategy,
            "deterministically minimized under the configured strategy"
        );
        // Findings carry Phase-3-ready identity, expectation and observation.
        assert!(!f.identity.case_id.is_empty());
        assert_eq!(f.identity.wasm_hash.len(), 64);
        assert!(!f.identity.auth_mode.is_empty());
    }

    // Same seed ⇒ byte-identical finding identities and reason codes.
    let (_, lines2) = campaign_result([21u8; 32]);
    assert_eq!(lines, lines2, "same seed ⇒ same campaign output");

    // Different seed ⇒ a different generation stream. The counter spec's
    // functions carry no parameters, so campaign *case ids* are positional
    // and identical by design; the divergence lives in generated values.
    // Verify directly on the generator with a value-carrying function set.
    let spec = sdkt_wasm::parse_contract_spec(AUTH_WASM).unwrap();
    let a = sdkt_fuzz::generator::generate_call(
        &spec,
        "set",
        &[21u8; 32],
        "c",
        GenerationCaps::default(),
    )
    .unwrap();
    let b = sdkt_fuzz::generator::generate_call(
        &spec,
        "set",
        &[22u8; 32],
        "c",
        GenerationCaps::default(),
    )
    .unwrap();
    assert_ne!(a.args, b.args, "different seed ⇒ different generated input");
    assert_ne!(a.generation_id, b.generation_id);
}

// ---------------------------------------------------------------------------
// Determinism: mutation identity + generation identity are stable
// ---------------------------------------------------------------------------

#[test]
fn mutation_and_generation_identity_is_stable_and_seed_bound() {
    let seed = [5u8; 32];
    let id1 = sdkt_fuzz::mutation::mutation_id(&seed, "case", 0, Operator::Zero);
    let id2 = sdkt_fuzz::mutation::mutation_id(&seed, "case", 0, Operator::Zero);
    assert_eq!(id1, id2);
    assert_ne!(
        id1,
        sdkt_fuzz::mutation::mutation_id(&seed, "case", 1, Operator::Zero),
        "arg index binds identity"
    );
    assert_ne!(
        id1,
        sdkt_fuzz::mutation::mutation_id(&seed, "case", 0, Operator::One),
        "operator binds identity"
    );
    assert_ne!(
        id1,
        sdkt_fuzz::mutation::mutation_id(&[6u8; 32], "case", 0, Operator::Zero),
        "seed binds identity"
    );

    let spec = sdkt_wasm::parse_contract_spec(COUNTER_WASM).unwrap();
    let g1 =
        sdkt_fuzz::generator::generate_call(&spec, "hello", &seed, "c", GenerationCaps::default())
            .unwrap();
    let g2 =
        sdkt_fuzz::generator::generate_call(&spec, "hello", &seed, "c", GenerationCaps::default())
            .unwrap();
    assert_eq!(g1, g2);
    let g3 = sdkt_fuzz::generator::generate_call(
        &spec,
        "hello",
        &seed,
        "other",
        GenerationCaps::default(),
    )
    .unwrap();
    assert_ne!(g1.generation_id, g3.generation_id);

    // Identities contain no timestamps, addresses or debug strings: they are
    // lowercase hex digests only.
    assert!(
        id1.chars().all(|c| c.is_ascii_hexdigit()),
        "id must be pure hex"
    );
    assert!(g1.generation_id.chars().all(|c| c.is_ascii_hexdigit()));
}

// ---------------------------------------------------------------------------
// U. campaign sequence execution (sequence_every > 0): state carry, multi-step
//    artifact recording, determinism, and replay reproduction
// ---------------------------------------------------------------------------

#[test]
fn campaign_with_sequence_every_produces_multistep_artifacts_that_replay() {
    let mut input = sdkt_fuzz::CampaignInput::new(COUNTER_WASM, [29u8; 32]).unwrap();
    input.cases = 4;
    input.function = Some("increment".to_string());
    input.sequence_length = 3;
    input.sequence_every = 2;

    // `increment()` starts from 0 on each fresh case and adds 1 to instance
    // storage per call. Declaring `expect_return = Some(U32(1))` means:
    // - single-step cases (case0001, case0003) return U32(1) -> PASS;
    // - 3-step sequence cases (case0000, case0002) carry state across steps
    //   and return U32(3) -> FINDING (RETURN_MISMATCH), which minimizes to a
    //   2-step sequence `[increment, increment]` returning U32(2) (removing
    //   step 1 would return U32(1) and lose the finding).
    let mut expectations = BTreeMap::new();
    expectations.insert(
        "increment".to_string(),
        Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: Some(ScVal::U32(1)),
            },
            ..Default::default()
        },
    );

    let (result, artifacts) = run_campaign_artifacts(&input, &expectations).unwrap();
    assert_eq!(result.executed, 4);
    assert_eq!(
        sequence_case_counts(result.executed, input.sequence_length, input.sequence_every),
        (2, 2)
    );
    assert_eq!(
        result.passed,
        vec!["case0001".to_string(), "case0003".to_string()],
        "single-step cases return 1 and pass"
    );
    assert_eq!(
        result.findings.len(),
        2,
        "only the multi-step sequence cases violate expect_return=1"
    );
    assert_eq!(artifacts.len(), 2);

    for (idx, art) in artifacts.iter().enumerate() {
        let expected_case_id = format!("case{:04}", idx * 2);
        assert_eq!(art.case_id, expected_case_id);
        assert_eq!(art.reason_code, "RETURN_MISMATCH");
        assert_eq!(art.campaign.sequence_length, 3);
        assert_eq!(art.campaign.sequence_every, 2);
        assert_eq!(
            art.sequence.len(),
            2,
            "artifact must record the multi-step sequence that carries state"
        );
        assert!(art.sequence.iter().all(|s| s.function == "increment"));
        assert_eq!(art.steps().unwrap().len(), 2);

        let outcome = replay(art, COUNTER_WASM).expect("multi-step artifact replays");
        assert!(
            outcome.is_reproduced(),
            "replay must reproduce the multi-step sequence finding: {outcome:?}"
        );
    }

    // Determinism: running the same sequence-enabled campaign again yields
    // byte-identical artifacts and canonical hashes.
    let (_, artifacts_again) = run_campaign_artifacts(&input, &expectations).unwrap();
    assert_eq!(artifacts, artifacts_again);
}

#[test]
fn runner_writes_and_replays_multistep_artifacts_when_sequence_enabled() {
    // Patch the overflow check in `COUNTER_WASM`'s `increment()` from
    // `u32::MAX` (`42 ff ff ff ff 0f`) to `1` (`42 81 80 80 80 00`, same
    // 6-byte LEB128 width) so the first call succeeds (0 -> 1) and the second
    // call in a sequence traps on the carried state (1 == 1 -> unreachable).
    let needle = [0x42, 0xff, 0xff, 0xff, 0xff, 0x0f];
    let replacement = [0x42, 0x81, 0x80, 0x80, 0x80, 0x00];
    let mut stateful_wasm = COUNTER_WASM.to_vec();
    let pos = stateful_wasm
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("counter overflow check present in fixture");
    stateful_wasm[pos..pos + needle.len()].copy_from_slice(&replacement);

    let dir = std::env::temp_dir().join(format!("sdkt-fuzz-seq-runner-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wasm_path = dir.join("stateful_counter.wasm");
    std::fs::write(&wasm_path, &stateful_wasm).unwrap();
    let artifact_dir = dir.join("artifacts");

    let options = FuzzRunOptions {
        wasm: wasm_path.clone(),
        seed: 7,
        cases: 4,
        function: Some("increment".to_string()),
        artifact_dir: Some(artifact_dir.clone()),
        auth_mode: sdkt_fuzz::AuthMode::NoAuth,
        expect_success: vec!["increment".to_string()],
        expect_error: Vec::new(),
        // Default-off: this test exercises the sequence path, not the
        // RESOURCE_LIMIT rule or capped budgets.
        resource_limit_is_finding: false,
        budget_capped: None,
        replay: true,
    };

    let report = sdkt_fuzz::run_fuzz_with_sequence(
        &options,
        SequenceOptions {
            sequence_length: 3,
            sequence_every: 2,
        },
    )
    .expect("sequence campaign runs");
    assert_eq!(
        report.summary,
        "executed=4 passed=2 expected_errors=0 findings=2 skipped=0 sequences=2 single_step=2"
    );
    assert!(report.evidence.deterministic);
    assert_eq!(report.evidence.config.sequence_length, 3);
    assert_eq!(report.evidence.config.sequence_every, 2);
    assert_eq!(report.artifacts.len(), 2);
    assert!(report.evidence.all_reproduced());

    for (file_name, _) in &report.artifacts {
        let art_path = artifact_dir.join(file_name);
        let art = sdkt_fuzz::load_artifact(&art_path).expect("artifact loads from disk");
        assert_eq!(art.sequence.len(), 2, "multi-step sequence written to disk");
        assert_eq!(art.campaign.sequence_length, 3);
        assert_eq!(art.campaign.sequence_every, 2);

        let replay_outcome = sdkt_fuzz::run_replay(&ReplayOptions {
            artifact: art_path,
            wasm: wasm_path.clone(),
        })
        .expect("disk replay succeeds");
        assert!(replay_outcome.is_reproduced());
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Unused-binding guard for optional imports.
#[allow(dead_code)]
fn _unused(_: Option<Mutation>) {}

// ---------------------------------------------------------------------------

/// `set(n: u32)` echoes its argument; declaring a return of 43 makes every
/// mutated case a finding, and two mutations are applied per case.
fn chain_campaign() -> (sdkt_fuzz::CampaignResult, Vec<sdkt_fuzz::FindingArtifact>) {
    let mut input = sdkt_fuzz::CampaignInput::new(AUTH_WASM, [9u8; 32]).unwrap();
    input.cases = 4;
    input.mutations_per_case = 2;
    input.sequence_every = 0;
    input.function = Some("set".to_string());
    let mut expectations = BTreeMap::new();
    expectations.insert(
        "set".to_string(),
        Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: Some(ScVal::U32(43)),
            },
            ..Default::default()
        },
    );
    run_campaign_artifacts(&input, &expectations).unwrap()
}

// Mutation chain: every applied mutation is recorded, in order (issue #500)
// ---------------------------------------------------------------------------

#[test]
fn two_mutation_finding_records_full_chain_in_order() {
    let (result, _) = chain_campaign();
    let finding = result
        .findings
        .iter()
        .find(|f| f.mutations.len() == 2)
        .expect("a finding produced by two mutations");
    assert_eq!(finding.identity.mutation_ids.len(), 2);
    assert_eq!(finding.identity.operators.len(), 2);
    for (i, m) in finding.mutations.iter().enumerate() {
        assert_eq!(finding.identity.mutation_ids[i], m.mutation_id);
        assert_eq!(finding.identity.operators[i], m.operator.name());
    }
    assert_ne!(
        finding.mutations[0].mutation_id,
        finding.mutations[1].mutation_id
    );
    // legacy single field is the last applied mutation
    assert_eq!(
        finding.mutation.as_ref().unwrap().mutation_id,
        finding.mutations[1].mutation_id
    );
}

#[test]
fn artifact_serializes_full_mutation_chain() {
    let (result, artifacts) = chain_campaign();
    let (f, a) = result
        .findings
        .iter()
        .zip(artifacts.iter())
        .find(|(f, _)| f.mutations.len() == 2)
        .expect("two-mutation finding");
    assert_eq!(a.mutations.len(), 2);
    for (i, m) in f.mutations.iter().enumerate() {
        assert_eq!(a.mutations[i].mutation_id, m.mutation_id);
    }
    let json = serde_json::to_string(a).unwrap();
    let back: sdkt_fuzz::FindingArtifact = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, a);
}
