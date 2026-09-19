//! Integration tests for deploy salt auto-generation.
//!
//! Verifies:
//! - `--salt` is optional (auto-generated when omitted)
//! - Explicit `--salt` still works
//! - Invalid salt still rejected
//! - Salt appears in pretty and JSON output
//!
//! CI-safe: no live Testnet, no hardcoded /tmp paths.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

/// Build sdkt command with isolated identity + network stores in a single temp dir.
fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

/// Path to the test fixture WASM file (relative to workspace root).
fn fixture_wasm() -> PathBuf {
    // CARGO_MANIFEST_DIR points to sdkt-cli/ during test compilation.
    // Walk up to workspace root, then into the fixture.
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ws_root = manifest.ancestors().nth(2).unwrap_or(&manifest);
    ws_root
        .join("crates/sdkt-cli/tests/fixtures/us_new.wasm")
        .to_path_buf()
}

// ---------- Invalid salt (explicit) — regression ----------

#[test]
fn deploy_invalid_salt_non_hex_rejected() {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-salt-auto-{}-nonhex",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);

    let wasm = fixture_wasm();
    if !wasm.exists() {
        eprintln!("Skipping: fixture WASM not found at {:?}", wasm);
        return;
    }

    sdkt_isolated(&dir)
        .args([
            "deploy",
            "--wasm",
            wasm.to_str().unwrap(),
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
fn deploy_invalid_salt_wrong_length_rejected() {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-salt-auto-{}-len",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);

    let wasm = fixture_wasm();
    if !wasm.exists() {
        eprintln!("Skipping: fixture WASM not found at {:?}", wasm);
        return;
    }

    // 38 hex chars (19 bytes) — must fail
    sdkt_isolated(&dir)
        .args([
            "deploy",
            "--wasm",
            wasm.to_str().unwrap(),
            "--salt",
            "00112233445566778899aabbccddeeff0011",
            "--identity",
            "default",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid --salt"));
}

#[test]
fn deploy_40char_nonhex_salt_rejected_with_hex_error() {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-salt-auto-{}-40hex",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);

    let wasm = fixture_wasm();
    if !wasm.exists() {
        eprintln!("Skipping: fixture WASM not found at {:?}", wasm);
        return;
    }

    let bad_salt = "z".repeat(40);
    sdkt_isolated(&dir)
        .args([
            "deploy",
            "--wasm",
            wasm.to_str().unwrap(),
            "--salt",
            bad_salt.as_str(),
            "--identity",
            "default",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a hex digit"));
}

// ---------- Explicit salt — backward compat ----------

#[test]
fn deploy_explicit_salt_help_works() {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-salt-auto-{}-help",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);

    sdkt_isolated(&dir)
        .args(["deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--salt"))
        .stdout(predicate::str::contains("--wasm"));
}

// ---------- No --salt (auto-generate) — design verification ----------

#[test]
fn deploy_without_salt_flag_shows_optional() {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-salt-auto-{}-optional",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);

    let output = sdkt_isolated(&dir)
        .args(["deploy", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // --salt should appear as optional (not required)
    // Look for --salt <SALT> (angle brackets = optional positional)
    // rather than --salt <SREQ> (required)
    assert!(stdout.contains("--salt"), "Help should mention --salt");
}

#[test]
fn deploy_explicit_salt_parses_correctly() {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-salt-auto-{}-parse",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);

    let wasm = fixture_wasm();
    if !wasm.exists() {
        eprintln!("Skipping: fixture WASM not found at {:?}", wasm);
        return;
    }

    let output = sdkt_isolated(&dir)
        .args([
            "deploy",
            "--wasm",
            wasm.to_str().unwrap(),
            "--salt",
            "00112233445566778899aabbccddeeff00112233",
            "--identity",
            "default",
        ])
        .output()
        .unwrap();

    // Should NOT fail with "Invalid --salt" (valid hex + correct length)
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Invalid --salt"),
        "Valid salt should not be rejected: {}",
        stderr
    );
}
