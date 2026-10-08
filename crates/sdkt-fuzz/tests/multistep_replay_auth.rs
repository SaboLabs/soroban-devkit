//! Regression tests for multi-step replay authorization (follow-up fix).
//!
//! Before the fix, `replay()` ran multi-step artifacts through
//! `execute_sequence` with NO auth entries, so a CorrectAuth artifact whose
//! execution depended on authorization replayed under a different
//! authorization posture than the campaign used. Single-step artifacts
//! already replayed with rebuilt auth entries; this closes the multi-step
//! gap with the same semantics (`auth_for_steps`: one entry list per step,
//! root = that step's call).
//!
//! Fixture: `auth_probe.wasm` — `bump(who)` calls `who.require_auth()`,
//! `set`/`peek` are unguarded. `bump(source_address)` succeeds only with a
//! matching source-account credential (empirically verified host 28.0.2
//! behavior). No auth-tree/multi-signer semantics are claimed.

use sdkt_fuzz::{
    artifact::CampaignConfig, replay, AuthMode, Environment, ExecutionStatus, Expected,
    ExpectedBehavior, Finding, FindingArtifact, FunctionCall, Oracle, ReasonCode,
};
use soroban_env_host::xdr::ScVal;

const AUTH_WASM: &[u8] = include_bytes!("fixtures/auth_probe.wasm");

fn campaign_config() -> CampaignConfig {
    let input = sdkt_fuzz::CampaignInput::new(AUTH_WASM, [42u8; 32]).unwrap();
    input.artifact_config()
}

/// Build a multi-step artifact exactly as the campaign would, but with an
/// explicit step sequence (the campaign's own `auth_for_steps` semantics
/// applied via `execute_sequence_auth`).
fn artifact_for(mode: AuthMode, expected_return: u32) -> FindingArtifact {
    let exec = sdkt_fuzz::Executor::new(AUTH_WASM, Default::default()).unwrap();
    let env = Environment::default();
    let who = ScVal::Address(sdkt_fuzz::auth::source_address());
    let steps = vec![
        FunctionCall::new("bump", vec![who.clone()]),
        FunctionCall::new("bump", vec![who]),
    ];
    let auth_per_step: Vec<Vec<Vec<u8>>> = if mode == AuthMode::NoAuth {
        Vec::new()
    } else {
        steps
            .iter()
            .enumerate()
            .map(|(i, call)| {
                let contract = exec
                    .case(format!("seq/step{i}"), call.clone(), Vec::new())
                    .contract_address();
                sdkt_fuzz::auth::invoke_auth_entries(mode, &contract, call).unwrap()
            })
            .collect()
    };
    let run =
        sdkt_fuzz::execute_sequence_auth(&exec, &[], &env, &steps, "seq", &auth_per_step).unwrap();
    let obs = run.final_observation;

    let expected = Expected {
        behavior: ExpectedBehavior::Success {
            expect_return: Some(ScVal::U32(expected_return)),
        },
        ..Default::default()
    };
    let oracle = Oracle::with_auth_mode(expected.clone(), mode);
    let classification = oracle.classify(&obs);
    let reason = match &classification {
        Classification::Finding(r) => *r,
        other => panic!("expected a finding under {mode:?}, got {other:?}"),
    };
    let case = exec.case("seq", steps[0].clone(), Vec::new());
    let finding = Finding::new(
        reason,
        case,
        None,
        env,
        expected,
        obs,
        exec.wasm_hash(),
        mode,
    );
    FindingArtifact::from_finding(&finding, campaign_config(), &steps, AUTH_WASM)
}

use sdkt_fuzz::Classification;

/// CorrectAuth, two authenticated `bump` calls ⇒ final returns 2; the rule
/// demands 1 ⇒ RETURN_MISMATCH. Pre-fix replay dropped the auth entries, the
/// final `bump` failed with the host's Auth error, and the recorded finding
/// could NOT be reproduced — this test fails on the old code.
#[test]
fn multistep_correctauth_artifact_reproduces() {
    let artifact = artifact_for(AuthMode::CorrectAuth, 1);
    assert_eq!(artifact.reason_code, "RETURN_MISMATCH");
    assert!(matches!(
        artifact.observation.status,
        sdkt_fuzz::artifact::StatusSnapshot::Returned { .. }
    ));

    let outcome = replay(&artifact, AUTH_WASM).expect("replay runs");
    assert!(
        outcome.is_reproduced(),
        "CorrectAuth multi-step artifact must replay under the same auth posture: {outcome:?}"
    );
    match outcome {
        sdkt_fuzz::ReplayOutcome::Reproduced { reason_code, .. } => {
            assert_eq!(reason_code, ReasonCode::ReturnMismatch);
        }
        other => panic!("expected Reproduced, got {other:?}"),
    }
}

/// WrongAuth: the guarded sequence fails with the host's Auth error in both
/// the campaign and the replay, and the recorded finding is an
/// UNEXPECTED_ERROR (a success was declared). Replay must classify the
/// re-executed sequence consistently with the campaign's recorded reason.
#[test]
fn multistep_wrongauth_artifact_reproduces() {
    let artifact = artifact_for(AuthMode::WrongAuth, 0);
    assert_eq!(artifact.reason_code, "UNEXPECTED_ERROR");
    assert!(matches!(
        artifact.observation.status,
        sdkt_fuzz::artifact::StatusSnapshot::ContractError { .. }
    ));
    let outcome = replay(&artifact, AUTH_WASM).expect("replay runs");
    assert!(
        outcome.is_reproduced(),
        "WrongAuth multi-step replay must be consistent with the campaign: {outcome:?}"
    );
}

/// NoAuth stays NoAuth: replaying a NoAuth multi-step artifact must
/// reproduce, and its bytes are unaffected by the fix (no entries were ever
/// sent on either side).
#[test]
fn multistep_noauth_artifact_reproduces() {
    let artifact = artifact_for(AuthMode::NoAuth, 0);
    assert_eq!(artifact.auth_mode, "no_auth");
    assert_eq!(artifact.reason_code, "UNEXPECTED_ERROR");
    let outcome = replay(&artifact, AUTH_WASM).expect("replay runs");
    assert!(
        outcome.is_reproduced(),
        "NoAuth multi-step replay must be unchanged: {outcome:?}"
    );
}

/// Single-step behavior is unchanged: a CorrectAuth-authorized single bump
/// replayed from a hand-built single-step artifact still reproduces.
#[test]
fn singlestep_correctauth_behavior_unchanged() {
    let exec = sdkt_fuzz::Executor::new(AUTH_WASM, Default::default()).unwrap();
    let env = Environment::default();
    let who = ScVal::Address(sdkt_fuzz::auth::source_address());
    let call = FunctionCall::new("bump", vec![who]);
    let contract = exec.case("s", call.clone(), Vec::new()).contract_address();
    let entries =
        sdkt_fuzz::auth::invoke_auth_entries(AuthMode::CorrectAuth, &contract, &call).unwrap();
    let obs = exec
        .execute_with_auth(&exec.case("s", call.clone(), Vec::new()), &env, &entries)
        .unwrap();
    assert!(matches!(obs.status, ExecutionStatus::Returned(_)));

    let expected = Expected {
        behavior: ExpectedBehavior::Success {
            expect_return: Some(ScVal::U32(99)),
        },
        ..Default::default()
    };
    let oracle = Oracle::with_auth_mode(expected.clone(), AuthMode::CorrectAuth);
    let reason = match oracle.classify(&obs) {
        Classification::Finding(r) => r,
        other => panic!("expected RETURN_MISMATCH finding, got {other:?}"),
    };
    let finding = Finding::new(
        reason,
        exec.case("s", call.clone(), Vec::new()),
        None,
        env,
        expected,
        obs,
        exec.wasm_hash(),
        AuthMode::CorrectAuth,
    );
    let artifact = FindingArtifact::from_finding(&finding, campaign_config(), &[call], AUTH_WASM);
    let outcome = replay(&artifact, AUTH_WASM).expect("single-step replay runs");
    assert!(
        outcome.is_reproduced(),
        "single-step CorrectAuth replay behavior must not regress: {outcome:?}"
    );
}
