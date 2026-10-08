//! Phase 3 tests G + J: auth-mode campaign wiring + release-assurance evidence.
//!
//! The auth fixture (`bump` = require_auth, `set`, `peek`) exists so host
//! authorization behavior is observable through the public API. Everything
//! asserted below is *empirically verified* host behavior of
//! `soroban-env-host` 28.0.2 under its enforcing authorization manager:
//!
//! - NoAuth against a `require_auth` function fails with an `Auth` error;
//!   a campaign rule *declaring* that error classifies it EXPECTED_ERROR.
//! - A `SourceAccount` entry whose root matches the call authorizes it.
//! - An `AddressV2` entry with an invalid signature does not authorize it.
//!
//! NOT claimed and NOT observable through the public API (recorded, see the
//! Phase 3 report): unconsumed-entry reporting, auth-tree tracing,
//! multi-signer semantics.

use std::collections::BTreeMap;

use sdkt_fuzz::evidence::CampaignEvidence;
use sdkt_fuzz::{
    run_campaign_artifacts, AuthMode, CampaignInput, Classification, Environment, Expected,
    ExpectedBehavior, Oracle, ReplayStatus, SCHEMA_VERSION,
};

const AUTH_WASM: &[u8] = include_bytes!("fixtures/auth_probe.wasm");

/// Declared Auth/6 error for the guarded function, so auth failures become
/// EXPECTED_ERROR — not findings (ERROR ≠ vulnerability).
fn declared_auth_error() -> BTreeMap<String, Expected> {
    let mut m = BTreeMap::new();
    m.insert(
        "bump".to_string(),
        Expected {
            behavior: ExpectedBehavior::Error {
                error_type: "Auth".to_string(),
                code: 6,
            },
            ..Default::default()
        },
    );
    m
}

fn auth_campaign(
    auth_mode: AuthMode,
    expectations: &BTreeMap<String, Expected>,
) -> (sdkt_fuzz::CampaignResult, Vec<sdkt_fuzz::FindingArtifact>) {
    let mut input = CampaignInput::new(AUTH_WASM, [77u8; 32]).unwrap();
    input.cases = 6;
    input.mutations_per_case = 0; // deterministic argument vectors, no mutation noise
    input.auth_mode = auth_mode;
    run_campaign_artifacts(&input, expectations).unwrap()
}

// ---------------------------------------------------------------------------
// G. NoAuth campaign
// ---------------------------------------------------------------------------

#[test]
fn campaign_noauth_declares_auth_failure_as_expected_error() {
    // `bump` requires auth. The campaign runs NoAuth: every bump case must
    // fail with the *declared* Auth/6 error → EXPECTED_ERROR, never a
    // finding, never an artifact.
    let (result, artifacts) = auth_campaign(AuthMode::NoAuth, &declared_auth_error());
    assert_eq!(result.executed, 6);
    assert!(
        result.findings.is_empty(),
        "declared auth failures are not findings: {:?}",
        result
            .findings
            .iter()
            .map(|f| f.reason_code)
            .collect::<Vec<_>>()
    );
    assert!(
        artifacts.is_empty(),
        "EXPECTED_ERROR never produces artifacts"
    );
    assert!(
        !result.expected_errors.is_empty(),
        "bump-under-NoAuth cases must classify as EXPECTED_ERROR"
    );
    // The unguarded functions still ran and passed.
    assert!(!result.passed.is_empty());
}

#[test]
fn campaign_noauth_undeclared_auth_failure_is_unexpected_error_finding() {
    // Without the declared error rule, the same NoAuth `bump` failure
    // violates `Expected: Success` → UNEXPECTED_ERROR finding. The
    // declaration — not the engine — decides what is expected.
    let mut expectations = BTreeMap::new();
    expectations.insert(
        "bump".to_string(),
        Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: None,
            },
            ..Default::default()
        },
    );
    let (result, artifacts) = auth_campaign(AuthMode::NoAuth, &expectations);
    assert!(
        result
            .findings
            .iter()
            .any(|f| f.reason_code == sdkt_fuzz::ReasonCode::UnexpectedError),
        "an undeclared auth failure is an unexpected-error finding"
    );
    assert!(!artifacts.is_empty(), "findings produce artifacts");
    assert_eq!(artifacts[0].schema_version, SCHEMA_VERSION);
    assert_eq!(artifacts[0].auth_mode, "no_auth");
    // The artifact replays.
    let outcome = sdkt_fuzz::replay(&artifacts[0], AUTH_WASM).unwrap();
    assert!(
        outcome.is_reproduced(),
        "the finding artifact must reproduce: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// G. CorrectAuth campaign
// ---------------------------------------------------------------------------

#[test]
fn campaign_correctauth_authorizes_matched_invocations() {
    // `set`/`peek` do not require auth: they succeed under CorrectAuth too.
    // `bump` requires the source account's auth; the campaign supplies a
    // SourceAccount entry rooted at that exact call, so it must also
    // succeed. All cases pass; nothing is expected to error.
    let (result, artifacts) = auth_campaign(AuthMode::CorrectAuth, &BTreeMap::new());
    assert_eq!(result.executed, 6);
    assert!(artifacts.is_empty());
    assert!(result.findings.is_empty());
    assert!(result.expected_errors.is_empty(), "{:?}", result);
    assert_eq!(result.passed.len(), 6, "every case succeeds");
}

// ---------------------------------------------------------------------------
// G. WrongAuth campaign
// ---------------------------------------------------------------------------

#[test]
fn campaign_wrongauth_does_not_authorize_guarded_calls() {
    // The AddressV2 entry with the fixed all-zero signature cannot
    // authenticate `bump`; the guarded cases must fail with the declared
    // Auth/6 error. Unguarded functions (`set`, `peek`) are unaffected.
    //
    // Honest scope note: the host does not report *unconsumed* entries, so
    // for the unguarded functions the entry simply goes unused — that is
    // host behavior, not modeled as evidence here.
    let (result, artifacts) = auth_campaign(AuthMode::WrongAuth, &declared_auth_error());
    assert_eq!(result.executed, 6);
    assert!(
        !result.expected_errors.is_empty(),
        "bump-under-WrongAuth must classify as the declared EXPECTED_ERROR"
    );
    assert!(artifacts.is_empty());
    assert!(result.findings.is_empty());
    assert!(
        !result.passed.is_empty(),
        "unguarded functions still succeed"
    );
}

#[test]
fn campaign_wrongauth_guarded_success_would_be_a_finding_only_when_declared() {
    // If a rule declared Success for the guarded function, a WrongAuth
    // *failure* violates it → UNEXPECTED_ERROR. This pins the wiring: the
    // mode reaches the executor and the oracle reacts to what the host
    // actually did.
    let mut expectations = BTreeMap::new();
    expectations.insert(
        "bump".to_string(),
        Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: None,
            },
            ..Default::default()
        },
    );
    let (result, artifacts) = auth_campaign(AuthMode::WrongAuth, &expectations);
    assert!(result
        .findings
        .iter()
        .any(|f| f.reason_code == sdkt_fuzz::ReasonCode::UnexpectedError));
    assert!(artifacts.iter().all(|a| a.auth_mode == "wrong_auth"));
    for a in &artifacts {
        let o = sdkt_fuzz::replay(a, AUTH_WASM).unwrap();
        assert!(o.is_reproduced(), "wrong-auth findings must replay: {o:?}");
    }
}

#[test]
fn campaign_auth_mode_is_recorded_in_config_and_artifacts() {
    for mode in [AuthMode::NoAuth, AuthMode::CorrectAuth, AuthMode::WrongAuth] {
        let input = {
            let mut i = CampaignInput::new(AUTH_WASM, [5u8; 32]).unwrap();
            i.auth_mode = mode;
            i
        };
        let cfg = input.artifact_config();
        assert_eq!(cfg.auth_modes, vec![mode.name().to_string()]);
    }
    let _ = (
        Environment::default(),
        Classification::Pass,
        Oracle::default(),
    );
}

// ---------------------------------------------------------------------------
// J. release-assurance evidence
// ---------------------------------------------------------------------------

#[test]
fn evidence_reports_a_completed_campaign() {
    let (result, artifacts) = auth_campaign(AuthMode::NoAuth, &declared_auth_error());
    let input = {
        let mut i = CampaignInput::new(AUTH_WASM, [77u8; 32]).unwrap();
        i.cases = 6;
        i.mutations_per_case = 0;
        i.auth_mode = AuthMode::NoAuth;
        i
    };
    let config = input.artifact_config();

    // No replay yet.
    let ev =
        CampaignEvidence::from_result(&result, config.clone(), &artifacts, &BTreeMap::new(), true);
    assert!(ev.campaign_completed);
    assert_eq!(ev.case_count, 6);
    assert_eq!(ev.passed + ev.expected_errors + ev.findings.len(), 6);
    assert!(
        !ev.has_findings(),
        "declared auth failures are not findings"
    );
    assert!(!ev.artifacts_available);
    assert!(ev.deterministic);
    assert!(!ev.all_reproduced(), "nothing replayed yet");

    // With replayed artifacts.
    let mut replay = BTreeMap::new();
    for a in &artifacts {
        let outcome = sdkt_fuzz::replay(a, AUTH_WASM).unwrap();
        sdkt_fuzz::evidence::record_replay(&mut replay, &a.canonical_hash(), &outcome);
    }
    // No artifacts existed in this campaign; the map stays empty but the
    // evidence shape is proven by the next test.
    assert!(replay.is_empty());
    let _ = ev;
}

#[test]
fn evidence_with_findings_and_replay() {
    // Findings-producing campaign (undeclared Success for `bump`).
    let mut expectations = BTreeMap::new();
    expectations.insert(
        "bump".to_string(),
        Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: None,
            },
            ..Default::default()
        },
    );
    let (result, artifacts) = auth_campaign(AuthMode::NoAuth, &expectations);
    assert!(!artifacts.is_empty());

    let input = {
        let mut i = CampaignInput::new(AUTH_WASM, [77u8; 32]).unwrap();
        i.cases = 6;
        i.mutations_per_case = 0;
        i.auth_mode = AuthMode::NoAuth;
        i
    };
    let mut replay = BTreeMap::new();
    for a in &artifacts {
        let outcome = sdkt_fuzz::replay(a, AUTH_WASM).unwrap();
        sdkt_fuzz::evidence::record_replay(&mut replay, &a.canonical_hash(), &outcome);
    }
    let ev =
        CampaignEvidence::from_result(&result, input.artifact_config(), &artifacts, &replay, true);

    assert!(
        ev.has_findings(),
        "only Classification::Finding is evidence"
    );
    assert!(ev.artifacts_available);
    assert_eq!(ev.findings.len(), result.findings.len());
    for f in &ev.findings {
        assert_eq!(f.reason_code, "UNEXPECTED_ERROR");
        assert_eq!(f.artifact_hash.len(), 64, "canonical artifact hash");
        assert!(!f.case_id.is_empty());
    }
    assert!(ev.minimized_findings > 0);
    assert!(
        ev.all_reproduced(),
        "every artifact replayed and reproduced"
    );
    assert!(
        ev.replay
            .values()
            .all(|s| matches!(s, ReplayStatus::Reproduced { .. })),
        "{:?}",
        ev.replay
    );
}
