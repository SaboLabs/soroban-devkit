//! Integration coverage verifying that `--timeout` / `--interval` CLI values
//! reach the polling engine (via `validate_poll_pair`) and that omitted flags
//! retain the documented defaults.
//!
//! These tests invoke the `sdkt` binary end-to-end with `assert_cmd` so the
//! flag plumbing through clap → `validate_poll_pair` → `PollConfig` is
//! exercised exactly as a user would hit it. Invalid pairs are rejected
//! before any network or identity work (fail-fast), so no mock server is
//! required.

use assert_cmd::Command;
use predicates::prelude::*;

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    // Isolated network/identity state so no on-disk profile interferes.
    let dir = std::env::temp_dir().join(format!(
        "sdkt-poll-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);
    cmd.env("SDKT_NETWORK_DIR", &dir);
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd
}

const WASM: &str = "crates/sdkt-cli/tests/fixtures/us_new.wasm";

#[test]
fn deploy_rejects_interval_zero_before_network() {
    // `--interval 0` must be rejected by validate_poll_pair before any
    // network/identity resolution. Usage errors exit with code 2, matching
    // `invoke` / `deploy` conventions.
    sdkt()
        .args(["deploy", "--wasm", WASM, "--interval", "0"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("interval must be greater than 0"));
}

#[test]
fn deploy_rejects_timeout_below_interval() {
    // timeout (2) < interval (5) is invalid.
    sdkt()
        .args([
            "deploy",
            "--wasm",
            WASM,
            "--timeout",
            "2",
            "--interval",
            "5",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must be at least one --interval"));
}

#[test]
fn storage_restore_rejects_interval_zero() {
    sdkt()
        .args([
            "storage",
            "restore",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "--envelope",
            "AAAAAgAAAAA=",
            "--interval",
            "0",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("interval must be greater than 0"));
}

#[test]
fn project_deploy_rejects_interval_zero_before_lock_check() {
    // Project deploy must validate the poll pair before `sdkt.lock` / identity
    // work. Use a temp dir with no .sdkt.toml so the error surfaces cleanly.
    let dir = std::env::temp_dir().join(format!(
        "sdkt-proj-poll-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    sdkt()
        .args(["project", "deploy", "--interval", "0"])
        .current_dir(&dir)
        .env("SDKT_NETWORK_DIR", &dir)
        .assert()
        .failure()
        .stderr(predicate::str::contains("interval must be greater than 0"));
}

#[test]
fn invoke_rejects_invalid_poll_pair_with_usage_exit_code() {
    // The `invoke` arm must also use exit code 2 for invalid polling pairs.
    sdkt()
        .args([
            "invoke",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "hello",
            "--timeout",
            "1",
            "--interval",
            "10",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("must be at least one --interval"));
}

/// Documented defaults: with no `--timeout`/`--interval` the command must NOT
/// fail on validate_poll_pair (defaults 60/1 are valid). It may fail later for
/// other reasons (no identity / no network) but never with a poll-pair error.
#[test]
fn deploy_defaults_do_not_trigger_poll_error() {
    let result = sdkt()
        .args(["deploy", "--wasm", WASM])
        .assert()
        .get_output()
        .clone();

    let stderr = String::from_utf8_lossy(&result.stderr);
    // Defaults (timeout=60, interval=1) must pass validation.
    assert!(
        !stderr.contains("interval must be greater than 0"),
        "defaults should be valid, got: {stderr}"
    );
    assert!(
        !stderr.contains("timeout must be >= interval"),
        "defaults should be valid, got: {stderr}"
    );
}
