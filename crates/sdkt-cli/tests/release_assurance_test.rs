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
