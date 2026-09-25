//! Integration tests for the `sdkt deploy` post-deploy verification wiring ().
//!
//! CI-safe: these exercise flag parsing and the existing offline validation
//! only; no live network is contacted.

use assert_cmd::Command;
use predicates::prelude::*;

fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

#[test]
fn cli_deploy_help_displays_no_verify_flag() {
    let dir = tempfile::tempdir().unwrap();
    sdkt(dir.path())
        .args(["deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--no-verify"))
        .stdout(predicate::str::contains("verification"));
}

#[test]
fn cli_project_deploy_help_displays_no_verify_flag() {
    let dir = tempfile::tempdir().unwrap();
    sdkt(dir.path())
        .args(["project", "deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--no-verify"));
}

#[test]
fn cli_deploy_no_verify_still_validates_constructor_args() {
    // --no-verify must not short-circuit the existing offline validation: bad
    // constructor args still fail before any RPC call is attempted.
    let dir = tempfile::tempdir().unwrap();
    let wasm_file = dir.path().join("test.wasm");
    std::fs::write(&wasm_file, b"\0asm\x01\0\0\0").unwrap();

    sdkt(dir.path())
        .args([
            "deploy",
            "--wasm",
            wasm_file.to_str().unwrap(),
            "--identity",
            "default",
            "--no-verify",
            "--arg",
            "u32:not_a_number",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid u32 value"));
}

#[test]
fn cli_deploy_no_verify_rejects_missing_wasm_file() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.wasm");

    sdkt(dir.path())
        .args([
            "deploy",
            "--wasm",
            missing.to_str().unwrap(),
            "--identity",
            "default",
            "--no-verify",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error reading WASM file"));
}
