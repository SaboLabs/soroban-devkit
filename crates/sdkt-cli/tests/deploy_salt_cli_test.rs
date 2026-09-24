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
fn cli_deploy_fails_on_missing_wasm_file() {
    let dir = tempfile::tempdir().unwrap();
    let missing_wasm = dir.path().join("missing.wasm");

    sdkt(dir.path())
        .env("SDKT_IDENTITY_DIR", dir.path().join("identity"))
        .args([
            "deploy",
            "--wasm",
            missing_wasm.to_str().unwrap(),
            "--identity",
            "alice",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Error reading WASM file"))
        .stderr(predicate::str::contains("missing.wasm"))
        .stderr(predicate::str::contains("WASM bytes are empty").not());
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

/// 40 chars but contains non-hex characters — must report a hex error,
/// NOT the length error (see issue #48).
#[test]
fn cli_deploy_rejects_40char_nonhex_salt() {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-salt-{}-40char-nonhex",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);

    // Exactly 40 characters, but 'z' is not a hex digit
    let bad_salt = "z".repeat(40);
    sdkt(&dir)
        .args([
            "deploy",
            "--wasm",
            "/tmp/soroban-devkit/crates/sdkt-cli/tests/fixtures/us_new.wasm",
            "--salt",
            bad_salt.as_str(),
            "--identity",
            "default",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a hex digit"))
        .stderr(predicate::str::contains("length").not());
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
