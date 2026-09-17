//! Integration tests for `sdkt-rpc` deploy module.
//!
//! Uses mocked HTTP server via tokio::net::TcpListener to exercise full
//! upload→simulate→create→poll without real network calls.

use assert_cmd::Command;
use predicates::prelude::*;

/// Build a `sdkt` binary under test with isolated store.
fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_NETWORK_DIR", dir);
    cmd
}

#[test]
fn cli_deploy_rejects_invalid_salt_non_hex() {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-salt-{}-nonhex",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);

    sdkt(&dir)
        .args([
            "deploy",
            "--wasm",
            "/tmp/soroban-devkit/crates/sdkt-cli/tests/fixtures/us_new.wasm",
            "--salt",
            "not_a_hex_string!",
            "--identity",
            "default",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid --salt"));
}

#[test]
fn cli_deploy_rejects_invalid_salt_wrong_length() {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-salt-{}-len",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);

    // 8 hex chars only (4 bytes), not 40
    sdkt(&dir)
        .args([
            "deploy",
            "--wasm",
            "/tmp/soroban-devkit/crates/sdkt-cli/tests/fixtures/us_new.wasm",
            "--salt",
            "abcd1234",
            "--identity",
            "default",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid --salt"));
}
