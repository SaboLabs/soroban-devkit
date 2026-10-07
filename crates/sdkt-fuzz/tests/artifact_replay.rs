//! Phase 3 tests A–F: canonical artifact + deterministic replay.
//!
//! Same fixtures as Phase 2. The findings exercised here come from rules
//! *declared in this test*, so they are known fixture behavior under an
//! explicit oracle rule — never real-world vulnerability discovery.

use std::collections::BTreeMap;

use sdkt_fuzz::{
    replay, replay_json, run_campaign_artifacts, CampaignInput, Classification, ExecutionStatus,
    Executor, Expected, ExpectedBehavior, FunctionCall, ReasonCode,
};
use soroban_env_host::xdr::ScVal;

const COUNTER_WASM: &[u8] = include_bytes!("../../../crates/sdkt-cli/tests/fixtures/us_new.wasm");

/// Declared rule: `hello` must return 43 while it actually returns 42.
/// That is a RETURN_MISMATCH under the declaration.
fn hello_rule() -> BTreeMap<String, Expected> {
    let mut m = BTreeMap::new();
    m.insert(
        "hello".to_string(),
        Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: Some(ScVal::U32(43)),
            },
            ..Default::default()
        },
    );
    m
}

fn campaign(seed: [u8; 32]) -> (sdkt_fuzz::CampaignResult, Vec<sdkt_fuzz::FindingArtifact>) {
    let mut input = CampaignInput::new(COUNTER_WASM, seed).unwrap();
    input.cases = 4;
    input.mutations_per_case = 2;
    run_campaign_artifacts(&input, &hello_rule()).unwrap()
}

// ---------------------------------------------------------------------------
// A. artifact determinism
// ---------------------------------------------------------------------------

#[test]
fn artifact_deterministic_bytes_and_hash() {
    let (_, a) = campaign([31u8; 32]);
    let (_, b) = campaign([31u8; 32]);
    assert!(!a.is_empty(), "the declared rule must yield findings");
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(
            x.canonical_bytes(),
            y.canonical_bytes(),
            "canonical bytes must be stable"
        );
        assert_eq!(
            x.canonical_hash(),
            y.canonical_hash(),
            "canonical hash must be stable"
        );
        assert_eq!(x, y, "the artifact itself must round-trip identically");
    }
    // Canonical form is JSON with sorted keys and no insignificant space.
    let json = a[0].canonical_json();
    assert!(!json.contains('\n'), "canonical JSON has no newlines");
    assert!(!json.contains(": "), "canonical JSON has no spaced colons");
    assert_eq!(a[0].canonical_hash().len(), 64, "sha256 hex");
}

// ---------------------------------------------------------------------------
// B. artifact completeness
// ---------------------------------------------------------------------------

#[test]
fn artifact_is_self_contained_for_replay() {
    let (_, artifacts) = campaign([32u8; 32]);
    let art = &artifacts[0];

    assert_eq!(art.schema_version, sdkt_fuzz::SCHEMA_VERSION);
    assert_eq!(art.campaign.wasm_sha256.len(), 64, "wasm sha256 recorded");
    assert_eq!(art.seed().unwrap().len(), 32, "seed recoverable");
    assert!(!art.case_id.is_empty());
    assert_eq!(art.function, "hello");
    assert_eq!(art.auth_mode, "no_auth");
    assert_eq!(art.reason_code, "RETURN_MISMATCH");

    // Steps decode to runnable calls with typed args.
    let steps = art.steps().unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].function, "hello");

    // Environment and expectation round-trip.
    let env = art.environment();
    assert_eq!(env, sdkt_fuzz::Environment::default());
    assert_eq!(
        art.expected().behavior,
        ExpectedBehavior::Success {
            expect_return: Some(ScVal::U32(43))
        }
    );

    // Observation carries the schema-stable status, state and budget.
    assert!(matches!(
        art.observation.status,
        sdkt_fuzz::artifact::StatusSnapshot::Returned { .. }
    ));
    assert!(!art.observation.state.is_empty());
    assert!(art.minimization.attempted);

    // Round-trip through JSON keeps everything.
    let back = sdkt_fuzz::parse_artifact(&art.canonical_bytes()).unwrap();
    assert_eq!(&back, art);
}

// ---------------------------------------------------------------------------
// C. replay success
// ---------------------------------------------------------------------------

#[test]
fn replay_reproduces_the_finding() {
    let (_, artifacts) = campaign([33u8; 32]);
    let art = &artifacts[0];
    let outcome = replay(art, COUNTER_WASM).expect("replay runs");
    assert!(
        outcome.is_reproduced(),
        "artifact must reproduce its finding: {outcome:?}"
    );
    match outcome {
        sdkt_fuzz::ReplayOutcome::Reproduced { reason_code, .. } => {
            assert_eq!(reason_code, ReasonCode::ReturnMismatch);
        }
        other => panic!("expected Reproduced, got {other:?}"),
    }

    // Deterministic: repeat gives an identical verdict and hash.
    let again = replay(art, COUNTER_WASM).unwrap();
    assert_eq!(outcome, again);
    assert_eq!(outcome.canonical_hash(), again.canonical_hash());

    // The JSON path (what the CLI uses) agrees.
    let via_json = replay_json(&art.canonical_bytes(), COUNTER_WASM).unwrap();
    assert_eq!(via_json, outcome);
}

// ---------------------------------------------------------------------------
// E. wrong WASM is invalid input, never a finding
// ---------------------------------------------------------------------------

#[test]
fn wrong_wasm_is_invalid_input() {
    let (_, artifacts) = campaign([34u8; 32]);
    let art = &artifacts[0];
    let other_wasm = include_bytes!("fixtures/auth_probe.wasm");

    let err = replay(art, other_wasm).unwrap_err();
    match err {
        sdkt_fuzz::ReplayError::WasmHashMismatch { expected, actual } => {
            assert_eq!(expected, art.campaign.wasm_sha256);
            assert_ne!(actual, expected);
        }
        other => panic!("wrong wasm must be a hash mismatch, got {other:?}"),
    }

    // An empty/garbage WASM is the same class of error, not a finding.
    let err = replay(art, b"").unwrap_err();
    assert!(matches!(
        err,
        sdkt_fuzz::ReplayError::WasmHashMismatch { .. }
    ));
}

// ---------------------------------------------------------------------------
// F. modified artifact: rejected or mismatch, never silently passed
// ---------------------------------------------------------------------------

#[test]
fn modified_artifact_never_silently_succeeds() {
    let (_, artifacts) = campaign([35u8; 32]);
    let art = &artifacts[0];

    // 1. Tamper with the recorded reason code: replay must report a mismatch
    //    or an invalid-artifact error, never a reproduction.
    let mut tampered = art.clone();
    tampered.reason_code = "STATE_MISMATCH".to_string();
    match replay(&tampered, COUNTER_WASM) {
        Err(e) => assert!(matches!(
            e,
            sdkt_fuzz::ReplayError::InvalidArtifact(_)
                | sdkt_fuzz::ReplayError::Malformed(_)
                | sdkt_fuzz::ReplayError::WasmHashMismatch { .. }
        )),
        Ok(outcome) => assert!(
            !outcome.is_reproduced(),
            "a tampered reason code must not reproduce"
        ),
    }

    // 2. Tamper with the recorded expectation so the rule no longer matches
    //    the observation: replay must not report Reproduced.
    let mut tampered = art.clone();
    tampered.expected.behavior = sdkt_fuzz::artifact::BehaviorSnapshot::Success {
        expect_return: Some(sdkt_fuzz::artifact::encode_scval_xdr_hex(&ScVal::U32(42))),
    };
    let outcome = replay(&tampered, COUNTER_WASM).expect("well-formed artifact still replays");
    assert!(
        !outcome.is_reproduced(),
        "a softened expectation must not reproduce the original finding"
    );

    // 3. Corrupt the bytes: must fail as malformed input.
    let mut json = art.canonical_bytes();
    json.truncate(json.len() / 2);
    let err = replay_json(&json, COUNTER_WASM).unwrap_err();
    assert!(matches!(
        err,
        sdkt_fuzz::ReplayError::Malformed(_) | sdkt_fuzz::ReplayError::InvalidArtifact(_)
    ));

    // 4. Unknown schema version: rejected, never guessed.
    let mut bumped = art.clone();
    bumped.schema_version = 99;
    let err = replay_json(&bumped.canonical_bytes(), COUNTER_WASM).unwrap_err();
    assert!(matches!(
        err,
        sdkt_fuzz::ReplayError::UnsupportedSchema { found: 99, .. }
    ));

    // 5. Missing sequence steps: rejected as invalid.
    let mut empty = art.clone();
    empty.sequence.clear();
    let err = replay(&empty, COUNTER_WASM).unwrap_err();
    assert!(matches!(err, sdkt_fuzz::ReplayError::InvalidArtifact(_)));
}

// ---------------------------------------------------------------------------
// H. campaign determinism (finding + artifact stream)
// ---------------------------------------------------------------------------

#[test]
fn campaign_artifact_stream_is_deterministic() {
    let (ra, aa) = campaign([36u8; 32]);
    let (rb, ab) = campaign([36u8; 32]);
    assert_eq!(ra.passed, rb.passed);
    assert_eq!(ra.expected_errors, rb.expected_errors);
    assert_eq!(
        aa.iter().map(|a| a.canonical_hash()).collect::<Vec<_>>(),
        ab.iter().map(|a| a.canonical_hash()).collect::<Vec<_>>(),
        "same config + seed ⇒ identical artifact stream"
    );

    // Different seed ⇒ different stream (at least one artifact differs).
    let (_, ac) = campaign([37u8; 32]);
    assert_ne!(
        aa.iter().map(|a| a.canonical_hash()).collect::<Vec<_>>(),
        ac.iter().map(|a| a.canonical_hash()).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Sanity: only Findings become artifacts
// ---------------------------------------------------------------------------

#[test]
fn only_findings_produce_artifacts() {
    // No declared rule ⇒ everything is PASS ⇒ no artifacts at all.
    let mut input = CampaignInput::new(COUNTER_WASM, [38u8; 32]).unwrap();
    input.cases = 4;
    let (result, artifacts) = run_campaign_artifacts(&input, &BTreeMap::new()).unwrap();
    assert!(result.findings.is_empty());
    assert!(
        artifacts.is_empty(),
        "PASS/EXPECTED_ERROR never produce artifacts"
    );
    assert_eq!(result.passed.len(), 4);

    // EXPECTED_ERROR also produces no artifact.
    let mut input = CampaignInput::new(COUNTER_WASM, [39u8; 32]).unwrap();
    input.cases = 2;
    let mut expectations = BTreeMap::new();
    expectations.insert(
        "hello".to_string(),
        Expected {
            behavior: ExpectedBehavior::Any,
            ..Default::default()
        },
    );
    let (result, artifacts) = run_campaign_artifacts(&input, &expectations).unwrap();
    assert!(artifacts.is_empty());
    assert!(result.findings.is_empty());
    let _ = Classification::Pass;

    // And a finding from the executor path replays away from campaign state.
    let (_, arts) = campaign([40u8; 32]);
    let art = &arts[0];
    // Replay only needs the wasm bytes: nothing from the campaign survives.
    let outcome = sdkt_fuzz::replay(art, COUNTER_WASM).unwrap();
    assert!(outcome.is_reproduced());
    let _ = (
        Executor::new(COUNTER_WASM, Default::default()),
        FunctionCall::new("hello", vec![]),
        ExecutionStatus::Void,
    );
}
