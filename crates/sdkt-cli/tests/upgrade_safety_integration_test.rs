use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

fn fixture(name: &str) -> String {
    let dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/tests/fixtures/{}", dir, name)
}

#[test]
fn diff_help_documents_upgrade_safety() {
    sdkt()
        .args(["diff", "--help"])
        .assert()
        .success()
        .stdout(contains("upgrade-safety"));
}

#[test]
fn upgrade_safety_pretty_shows_breaking_and_nonbreaking() {
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
        ])
        .assert()
        .success()
        .stdout(contains("Upgrade Safety"))
        .stdout(contains("Compatible: NO"))
        .stdout(contains("Removed function: mint"))
        .stdout(contains("Removed function: transfer"))
        .stdout(contains("Added function: hello"));
}

#[test]
fn upgrade_safety_json_serializes_verdict() {
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(contains("\"compatible\":false"))
        .stdout(contains("\"breaking_changes\""))
        .stdout(contains("\"non_breaking_changes\""));
}

#[test]
fn deploy_deny_breaking_aborts_on_incompatible() {
    sdkt()
        .args([
            "deploy",
            "--wasm",
            &fixture("us_new.wasm"),
            "--salt",
            "0000000000000000000000000000000000000000",
            "--deny-breaking",
            "--old-wasm",
            &fixture("us_old.wasm"),
        ])
        .assert()
        .failure()
        .stderr(contains("NOT backwards-compatible"));
}

#[test]
fn deploy_fails_without_identity() {
    // Deploy without a configured identity should fail with identity error,
    // NOT with upgrade-safety guard error.
    sdkt()
        .args([
            "deploy",
            "--wasm",
            &fixture("us_new.wasm"),
            "--salt",
            "0000000000000000000000000000000000000001",
        ])
        .assert()
        .failure() // Expected to fail due to missing identity
        .stderr(contains("NOT backwards-compatible").not());
}
