use assert_cmd::Command;
use predicates::prelude::*;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn deploy(wasm: &str) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.args([
        "deploy",
        "--wasm",
        wasm,
        "--identity",
        "missing-preflight-test-identity",
    ]);
    cmd
}

#[test]
fn critical_audit_aborts_before_identity_and_upload() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("vuln.rs");
    std::fs::write(&src, "fn initialize() { let _ = 1; }\n").unwrap();
    deploy(&fixture("us_new.wasm"))
        .args([
            "--release-assurance",
            "--preflight-audit",
            src.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("Security        FAIL"))
        .stderr(predicate::str::contains(
            "Deployment aborted: release assurance failed",
        ))
        .stderr(predicate::str::contains("identity").not());
}

#[test]
fn passing_checks_continue_to_identity_without_on_chain_checks() {
    deploy(&fixture("us_new.wasm"))
        .arg("--release-assurance")
        .assert()
        .failure()
        .stdout(predicate::str::contains("Artifact        PASS"))
        .stdout(predicate::str::contains("Upgrade Safety  SKIPPED"))
        .stdout(predicate::str::contains("Verification    SKIPPED"))
        .stdout(predicate::str::contains("Contract Health SKIPPED"))
        .stderr(predicate::str::contains("identity"))
        .stderr(predicate::str::contains("Deployment aborted").not());
}

#[test]
fn passing_checks_send_pretty_report_to_stderr_in_json_mode() {
    deploy(&fixture("us_new.wasm"))
        .args(["--release-assurance", "--format", "json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("Soroban Release Assurance").not())
        .stderr(predicate::str::contains("Soroban Release Assurance"))
        .stderr(predicate::str::contains("Artifact        PASS"))
        .stderr(predicate::str::contains("identity"))
        .stderr(predicate::str::contains("Deployment aborted").not());
}

#[test]
fn critical_audit_aborts_to_stderr_in_json_mode() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("vuln.rs");
    std::fs::write(&src, "fn initialize() { let _ = 1; }\n").unwrap();
    deploy(&fixture("us_new.wasm"))
        .args([
            "--release-assurance",
            "--preflight-audit",
            src.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("Soroban Release Assurance").not())
        .stderr(predicate::str::contains("Soroban Release Assurance"))
        .stderr(predicate::str::contains("Security        FAIL"))
        .stderr(predicate::str::contains(
            "Deployment aborted: release assurance failed",
        ))
        .stderr(predicate::str::contains("identity").not());
}

#[test]
fn incompatible_upgrade_aborts_before_identity() {
    deploy(&fixture("us_new.wasm"))
        .args(["--release-assurance", "--old-wasm", &fixture("us_old.wasm")])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("Upgrade Safety  FAIL"))
        .stderr(predicate::str::contains("Deployment aborted"))
        .stderr(predicate::str::contains("identity").not());
}

#[test]
fn unreadable_artifact_and_bad_evidence_abort() {
    deploy("/missing-preflight-candidate.wasm")
        .arg("--release-assurance")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "Deployment aborted: release assurance error",
        ));
    let dir = tempfile::tempdir().unwrap();
    let evidence = dir.path().join("bad.json");
    std::fs::write(&evidence, "not json").unwrap();
    deploy(&fixture("us_new.wasm"))
        .args([
            "--release-assurance",
            "--preflight-fuzz-evidence",
            evidence.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "Deployment aborted: release assurance error",
        ))
        .stderr(predicate::str::contains("identity").not());
}

#[test]
fn no_flag_keeps_existing_path() {
    deploy(&fixture("us_new.wasm"))
        .assert()
        .failure()
        .stdout(predicate::str::contains("Soroban Release Assurance").not())
        .stderr(predicate::str::contains("identity"));
}

#[test]
fn help_describes_skipped_sections() {
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--release-assurance"))
        .stdout(predicate::str::contains("Contract Health"));
}
