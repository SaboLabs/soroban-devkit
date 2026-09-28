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

// ── diff --upgrade-safety --deny-breaking exit-code tests ────────────────────

/// `diff --upgrade-safety` without `--deny-breaking` exits 0 even when the
/// verdict is breaking (existing / flagless behaviour is unchanged).
#[test]
fn diff_upgrade_safety_no_flag_exits_zero_on_breaking() {
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
        .success() // exit 0 — no --deny-breaking flag
        .stdout(contains("Compatible: NO"));
}

/// `diff --upgrade-safety --deny-breaking` exits non-zero (2) when breaking
/// changes are present, and the full report is still printed before exit.
#[test]
fn diff_deny_breaking_exits_nonzero_on_incompatible() {
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
            "--deny-breaking",
        ])
        .assert()
        .failure() // exit non-zero (2)
        // Full report is still printed before exit.
        .stdout(contains("Upgrade Safety"))
        .stdout(contains("Compatible: NO"));
}

/// `diff --upgrade-safety --deny-breaking` exits 0 when the verdict is
/// compatible (comparing a WASM against itself — identical = no breaking changes).
#[test]
fn diff_deny_breaking_exits_zero_on_compatible() {
    // us_old.wasm vs itself: no changes at all → compatible verdict.
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_old.wasm"),
            "--upgrade-safety",
            "--deny-breaking",
        ])
        .assert()
        .success() // exit 0 — compatible
        .stdout(contains("Compatible: YES"));
}

// ── diff --help documents --deny-breaking ───────────────────────────────────

/// `sdkt diff --help` must advertise the new `--deny-breaking` flag so users
/// can discover it.
#[test]
fn diff_help_documents_deny_breaking() {
    sdkt()
        .args(["diff", "--help"])
        .assert()
        .success()
        .stdout(contains("deny-breaking"));
}

// ── verify --help documents --deny-breaking ─────────────────────────────────

/// `sdkt verify --help` must advertise `--deny-breaking`.
#[test]
fn verify_help_documents_deny_breaking() {
    sdkt()
        .args(["verify", "--help"])
        .assert()
        .success()
        .stdout(contains("deny-breaking"));
}

// ── health --help documents --fail-on ───────────────────────────────────────

/// `sdkt health --help` must advertise the new `--fail-on` flag.
#[test]
fn health_help_documents_fail_on() {
    sdkt()
        .args(["health", "--help"])
        .assert()
        .success()
        .stdout(contains("fail-on"));
}
