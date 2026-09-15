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
        .stdout(contains("Changed signature: mint()"))
        .stdout(contains("Added function: balance()"))
        .stdout(contains("Added event: Mint"));
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
        .stdout(contains("\"non_breaking_changes\""))
        .stdout(contains("\"changed_signature\""));
}

#[test]
fn deploy_deny_breaking_aborts_on_incompatible() {
    sdkt()
        .args([
            "deploy",
            "--wasm",
            &fixture("us_new.wasm"),
            "--salt",
            "salt",
            "--deny-breaking",
            "--old-wasm",
            &fixture("us_old.wasm"),
        ])
        .assert()
        .failure()
        .stderr(contains("NOT backwards-compatible"));
}

#[test]
fn deploy_without_deny_breaking_skips_guard() {
    // Without --deny-breaking the upgrade-safety guard is skipped entirely,
    // so it must never print the abort message (behavior unchanged).
    sdkt()
        .args([
            "deploy",
            "--wasm",
            &fixture("us_new.wasm"),
            "--salt",
            "salt",
        ])
        .assert()
        .success() // guard skipped, deploy proceeds (no abort)
        .stderr(contains("NOT backwards-compatible").not());
}
