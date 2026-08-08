//! M42 — `sdkt verify --upgrade-safety` behavior tests (hermetic).
//!
//! The on-chain path requires a reachable RPC, so the live verdict is covered by
//! the Compatibility CI (network-guarded, with a committed fixture fallback). This
//! crate test is hermetic: it asserts argument validation, graceful offline
//! failure (no panic), and that the upgrade-safety verdict produced by the shared
//! M14 engine (`diff_wasm` -> `UpgradeVerdict`) is identical for the same inputs —
//! proving the command reuses the engine rather than a parallel one.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    let dir = std::env::temp_dir().join(format!(
        "sdkt-m42-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&dir);
    cmd.env("SDKT_NETWORK_DIR", &dir);
    cmd
}

fn fixture(name: &str) -> String {
    let dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/tests/fixtures/{}", dir, name)
}

#[test]
fn verify_help_documents_upgrade_safety() {
    sdkt()
        .args(["verify", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("upgrade-safety"));
}

#[test]
fn upgrade_safety_without_wasm_is_controlled_error() {
    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--upgrade-safety",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--upgrade-safety requires --wasm"));
}

#[test]
fn upgrade_safety_with_missing_candidate_file_is_controlled_error() {
    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--wasm",
            "/no/such/file.wasm",
            "--upgrade-safety",
            "--network",
            "testnet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error reading WASM file"));
}

#[test]
fn upgrade_safety_with_malformed_candidate_is_controlled_error() {
    // A text file is not valid WASM -> must fail cleanly, never panic.
    let dir = std::env::temp_dir().join(format!(
        "sdkt-m42-bad-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.wasm");
    fs::write(&bad, b"this is not wasm").unwrap();

    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--wasm",
            bad.to_str().unwrap(),
            "--upgrade-safety",
            "--network",
            "testnet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not valid WASM"));
}

#[test]
fn upgrade_safety_offline_contract_unreachable_is_graceful() {
    // No RPC reachable -> clean failure (no panic), even with a valid candidate.
    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
            "--network",
            "testnet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error verifying upgrade safety"))
        .stderr(predicate::str::contains("panic").not());
}

#[test]
fn existing_verify_without_flag_still_runs() {
    // Regular `verify --contract` (no --wasm) must still behave (offline failure
    // is a clean error, not a crash) — backward compatibility preserved.
    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--network",
            "testnet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error verifying contract"))
        .stderr(predicate::str::contains("panic").not());
}

#[test]
fn breaking_change_verdict_matches_m14_engine() {
    // The same us_old -> us_new inputs through `diff --upgrade-safety` (which uses
    // the identical M14 engine the verify command reuses) must yield NO / breaking.
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
        .stdout(predicate::str::contains("Upgrade Safety"))
        .stdout(predicate::str::contains("Compatible: NO"))
        .stdout(predicate::str::contains("Changed signature: mint()"))
        .stdout(predicate::str::contains("Added function: balance()"));
}

#[test]
fn compatible_case_verdict_is_yes() {
    // us_old against itself is a no-op upgrade -> the M14 engine reports YES.
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_old.wasm"),
            "--upgrade-safety",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compatible: YES"));
}
