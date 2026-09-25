//! Integration tests for `sdkt project deploy --salt`.
//!
//! Verifies the flag is forwarded (validated) instead of silently discarded.
//! CI-safe: validation fails fast before any config, identity, or network work.

use assert_cmd::Command;
use predicates::prelude::*;

fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.current_dir(dir);
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-project-salt-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        tag
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn project_deploy_rejects_invalid_salt() {
    let dir = temp_dir("nonhex");
    sdkt_isolated(&dir)
        .args(["project", "deploy", "--salt", "not_a_hex_string!"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid --salt"));
}

#[test]
fn project_deploy_rejects_wrong_length_salt() {
    let dir = temp_dir("len");
    sdkt_isolated(&dir)
        .args(["project", "deploy", "--salt", "00112233"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must be 20-byte hex"));
}

#[test]
fn project_deploy_accepts_valid_salt() {
    let dir = temp_dir("valid");
    let output = sdkt_isolated(&dir)
        .args([
            "project",
            "deploy",
            "--salt",
            "00112233445566778899aabbccddeeff00112233",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Invalid --salt"),
        "Valid salt should not be rejected: {}",
        stderr
    );
}

#[test]
fn project_deploy_help_shows_salt_without_string_default() {
    let dir = temp_dir("help");
    sdkt_isolated(&dir)
        .args(["project", "deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--salt"))
        .stdout(predicate::str::contains("Auto-generated if omitted"))
        .stdout(predicate::str::contains("[default: deploy]").not());
}
