//! Release-assurance (`sdkt release-assurance`) regression tests.
//!
//! These tests exist to prove the composition is NOT hardcoded: each section's
//! status must come from a real existing engine (`sdkt_wasm::parse_metadata`,
//! `sdkt_wasm::upgrade_safety_wasm`, `sdkt_audit::audit_source_with`,
//! `verify_contract`, `contract_health`). If any check were replaced by a
//! constant `PASS`, the assertions below fail.

use assert_cmd::Command;
use predicates::str::contains;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

fn wasm_path(wasm: &[u8]) -> PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("candidate.wasm");
    std::fs::write(&path, wasm).expect("write wasm");
    // Leak the tempdir so the file survives for the spawned `sdkt` process.
    let path = path.to_path_buf();
    std::mem::forget(dir);
    path
}

/// Minimal valid WASM binary (magic + version 1) — the shape `parse_metadata`
/// accepts.
const MINIMAL_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

fn release_assurance(args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("release-assurance");
    cmd.args(args);
    cmd
}

// ---- A. invalid WASM → artifact FAIL → release FAIL ----

#[test]
fn invalid_wasm_fails_artifact_and_release() {
    let path = wasm_path(b"this is not a wasm file");
    release_assurance(&["--wasm", path.to_str().unwrap()])
        .assert()
        .failure()
        .stdout(contains("Artifact        FAIL"))
        .stdout(contains("RELEASE STATUS  FAIL"));
}

#[test]
fn invalid_wasm_json_reports_fail_not_hardcoded_pass() {
    let path = wasm_path(b"still not wasm");
    let out = release_assurance(&["--wasm", path.to_str().unwrap(), "--format", "json"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    assert_eq!(v["artifact"]["status"], "FAIL");
    assert_eq!(v["release_status"], "FAIL");
    // A failed artifact must not carry metadata (engine returned Err).
    assert!(v["artifact"]["metadata"].is_null());
}

// ---- B. incompatible previous WASM → real upgrade verdict FAIL ----

#[test]
fn breaking_upgrade_fails_on_real_verdict() {
    // us_old → us_new removes mint/transfer/Transfer/Point: the engine's own
    // verdict is incompatible, which must drive the section status.
    release_assurance(&[
        "--wasm",
        &fixture("us_new.wasm"),
        "--previous-wasm",
        &fixture("us_old.wasm"),
    ])
    .assert()
    .failure()
    .stdout(contains("Artifact        PASS"))
    .stdout(contains("Upgrade Safety  FAIL"))
    .stdout(contains("RELEASE STATUS  FAIL"));
}

#[test]
fn breaking_upgrade_json_carries_real_verdict() {
    let out = release_assurance(&[
        "--wasm",
        &fixture("us_new.wasm"),
        "--previous-wasm",
        &fixture("us_old.wasm"),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .get_output()
    .stdout
    .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");

    // Real engine output, not a constant.
    assert_eq!(v["upgrade_safety"]["verdict"]["compatible"], false);
    let breaking = v["upgrade_safety"]["verdict"]["breaking_changes"]
        .as_array()
        .expect("breaking_changes array");
    assert!(breaking.len() >= 3, "expected real breaking changes");
    let names: Vec<&str> = breaking.iter().filter_map(|c| c["name"].as_str()).collect();
    assert!(names.contains(&"mint"), "engine must report removed mint");
    assert!(
        names.contains(&"transfer"),
        "engine must report removed transfer"
    );
    assert_eq!(v["release_status"], "FAIL");

    // Artifact section must carry the real metadata hash of the candidate.
    let hash = v["artifact"]["metadata"]["hash"]
        .as_str()
        .expect("real metadata hash");
    assert_eq!(hash.len(), 64, "sha256 hex");
}

#[test]
fn compatible_upgrade_passes_the_section() {
    // Identical baseline → the engine reports compatible, so the section PASSes
    // (proves the status is derived, not constant-FAIL either).
    release_assurance(&[
        "--wasm",
        &fixture("us_new.wasm"),
        "--previous-wasm",
        &fixture("us_new.wasm"),
    ])
    .assert()
    .success()
    .stdout(contains("Upgrade Safety  PASS"))
    .stdout(contains("RELEASE STATUS  REVIEW"));
}

// ---- C. security finding → real audit severity → status changes ----

#[test]
fn audit_critical_finding_fails_security_and_release() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("vuln.rs");
    // AUTH-003 (Critical): initialize()-style entrypoint without require_auth.
    std::fs::write(&src, "fn initialize() { let _ = 1; }\n").expect("write source");

    release_assurance(&[
        "--wasm",
        &fixture("us_new.wasm"),
        "--audit",
        src.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .stdout(contains("Security        FAIL"))
    .stdout(contains("RELEASE STATUS  FAIL"));
}

#[test]
fn audit_warning_finding_reviews_security_not_pass() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("warn.rs");
    // MATH-001 (Warning): division before multiplication, no auth issue.
    std::fs::write(
        &src,
        "fn compute(a: i128, b: i128) -> i128 { (a / b) * a }\n",
    )
    .expect("write source");

    release_assurance(&[
        "--wasm",
        &fixture("us_new.wasm"),
        "--audit",
        src.to_str().unwrap(),
    ])
    .assert()
    .success()
    .stdout(contains("Security        REVIEW"))
    .stdout(contains("RELEASE STATUS  REVIEW"));
}

// ---- D. JSON serialization carries real structured results ----

#[test]
fn json_is_machine_readable_and_deterministic() {
    let out = release_assurance(&["--wasm", &fixture("us_new.wasm"), "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");

    assert_eq!(v["artifact"]["status"], "PASS");
    assert_eq!(v["security"]["status"], "SKIPPED");
    assert_eq!(v["upgrade_safety"]["status"], "SKIPPED");
    assert_eq!(v["verification"]["status"], "SKIPPED");
    assert_eq!(v["health"]["status"], "SKIPPED");
    assert_eq!(v["network"], "testnet");
    // No --contract supplied → the on-chain sections are explicit skips.
    assert!(v["verification"]["report"].is_null());
}

#[test]
fn skipped_onchain_sections_never_become_pass() {
    // SKIPPED must not be silently flattened into PASS: without --contract the
    // deployed reality is unknown, so the aggregate is REVIEW, not PASS.
    release_assurance(&["--wasm", &fixture("us_new.wasm")])
        .assert()
        .success()
        .stdout(contains("Verification    SKIPPED"))
        .stdout(contains("Contract Health SKIPPED"))
        .stdout(contains("RELEASE STATUS  REVIEW"));
}

// ---- E. aggregation is deterministic ----

#[test]
fn aggregation_fail_dominates() {
    // Artifact invalid overrides everything else, including skips.
    let path = wasm_path(b"garbage");
    release_assurance(&["--wasm", path.to_str().unwrap()])
        .assert()
        .failure()
        .stdout(contains("RELEASE STATUS  FAIL"));
}

#[test]
fn aggregation_review_when_only_checks_are_skipped() {
    release_assurance(&["--wasm", &fixture("us_new.wasm")])
        .assert()
        .success()
        .stdout(contains("RELEASE STATUS  REVIEW"));
}

#[test]
fn missing_required_wasm_argument_fails_to_parse() {
    release_assurance(&[]).assert().failure();
}

#[test]
fn nonexistent_wasm_path_is_a_command_error() {
    release_assurance(&["--wasm", "/no/such/contract.wasm"])
        .assert()
        .failure()
        .stderr(contains("Failed to read WASM file"));
}

#[test]
fn minimal_wasm_parses_through_the_real_metadata_engine() {
    let path = wasm_path(MINIMAL_WASM);
    let out = release_assurance(&["--wasm", path.to_str().unwrap(), "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    assert_eq!(v["artifact"]["status"], "PASS");
    assert_eq!(v["artifact"]["metadata"]["size_bytes"], 8);
    assert_eq!(v["artifact"]["metadata"]["version"], 1);
}

// ---- WASM size policy on release-assurance (opt-in) ----
//
// us_old.wasm = 198 bytes, us_new.wasm = 530 bytes. Policy violations fold
// into the ARTIFACT section (FAIL) which the existing aggregation turns into
// release_status=FAIL and exit 1. No policy flags ⇒ unchanged behavior.

#[test]
fn ra_abs_size_boundary_is_inclusive() {
    // Exactly at the limit passes.
    release_assurance(&["--wasm", &fixture("us_new.wasm"), "--max-size-bytes", "530"])
        .assert()
        .success();
    // One byte over fails, with the policy named in the report.
    let assert = release_assurance(&[
        "--wasm",
        &fixture("us_new.wasm"),
        "--max-size-bytes",
        "529",
        "--format",
        "json",
    ])
    .assert()
    .failure();
    let out = assert.get_output().stdout.clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    assert_eq!(v["artifact"]["status"], "FAIL");
    assert_eq!(v["release_status"], "FAIL");
    let detail = v["artifact"]["detail"].as_str().unwrap();
    assert!(detail.contains("size policy"), "{detail}");
    assert!(detail.contains("--max-size-bytes 529"), "{detail}");
    // Reasons surface the section failure through the existing loop.
    assert!(v["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r == "Artifact: FAIL"));
}

#[test]
fn ra_growth_policy_requires_previous_artifact() {
    // Requested check without a baseline must fail loudly, never be skipped.
    release_assurance(&["--wasm", &fixture("us_new.wasm"), "--max-growth-pct", "10"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "--max-growth-pct requires --previous-wasm",
        ));
}

#[test]
fn ra_growth_violation_blocks_release() {
    // us_old -> us_new grows 167.7% > 0%; upgrade-safety ALSO fails on this
    // pair (real ABI changes), so assert the policy fired in the artifact
    // section rather than relying on exit code alone.
    let assert = release_assurance(&[
        "--wasm",
        &fixture("us_new.wasm"),
        "--previous-wasm",
        &fixture("us_old.wasm"),
        "--max-growth-pct",
        "0",
        "--format",
        "json",
    ])
    .assert()
    .failure();
    let v: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    let detail = v["artifact"]["detail"].as_str().unwrap();
    assert!(detail.contains("size policy"), "{detail}");
    assert!(detail.contains("167.7%"), "{detail}");
    assert_eq!(v["artifact"]["status"], "FAIL");
    assert_eq!(v["release_status"], "FAIL");
}

#[test]
fn ra_zero_growth_identical_artifacts_passes_policy() {
    // Identical candidate and baseline: 0% growth is at any non-negative
    // threshold and the report stays REVIEW (on-chain sections skipped).
    let assert = release_assurance(&[
        "--wasm",
        &fixture("us_new.wasm"),
        "--previous-wasm",
        &fixture("us_new.wasm"),
        "--max-growth-pct",
        "0",
        "--format",
        "json",
    ])
    .assert()
    .success();
    let v: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    assert_eq!(v["artifact"]["status"], "PASS");
    assert_eq!(v["upgrade_safety"]["status"], "PASS");
    assert_eq!(v["release_status"], "REVIEW");
}

#[test]
fn ra_compatible_artifact_fails_only_on_size_policy() {
    // Same artifact twice: ABI verdict is PASS (identical), yet an absolute
    // size policy of 197 bytes (us_old = 198) blocks the release — proving
    // size policy and upgrade-safety are evaluated independently.
    let assert = release_assurance(&[
        "--wasm",
        &fixture("us_old.wasm"),
        "--previous-wasm",
        &fixture("us_old.wasm"),
        "--max-size-bytes",
        "197",
        "--format",
        "json",
    ])
    .assert()
    .failure();
    let v: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    // The size policy does not touch the ABI verdict: upgrade-safety ran
    // normally (the policy is applied post-hoc) and reported its own
    // comparison; only the artifact section is flipped by the size policy.
    assert_eq!(
        v["upgrade_safety"]["status"], "PASS",
        "ABI untouched by size policy"
    );
    assert_eq!(v["artifact"]["status"], "FAIL");
    assert_eq!(v["release_status"], "FAIL");
    let detail = v["artifact"]["detail"].as_str().unwrap();
    assert!(detail.contains("exceeds --max-size-bytes 197"), "{detail}");
}

#[test]
fn ra_no_policy_flags_keeps_artifact_unchanged() {
    // Regression guard: default artifact section identical to pre-policy.
    let assert = release_assurance(&["--wasm", &fixture("us_new.wasm"), "--format", "json"])
        .assert()
        .success();
    let v: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    assert_eq!(v["artifact"]["status"], "PASS");
    let detail = v["artifact"]["detail"].as_str().unwrap();
    assert!(detail.starts_with("valid WASM"), "{detail}");
    assert!(
        !detail.contains("size policy"),
        "no policy ⇒ no policy text: {detail}"
    );
    assert_eq!(v["release_status"], "REVIEW");
}

#[test]
fn ra_rejects_invalid_threshold_inputs() {
    release_assurance(&["--wasm", &fixture("us_new.wasm"), "--max-growth-pct", "NaN"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("finite"));
    release_assurance(&["--wasm", &fixture("us_new.wasm"), "--max-size-bytes", "0"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("greater than 0"));
}
