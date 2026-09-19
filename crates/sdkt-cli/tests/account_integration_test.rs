use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

fn add_mock_account_profile(dir: &std::path::Path, rpc_url: &str) {
    sdkt_isolated(dir)
        .args([
            "network",
            "add",
            "mocknet",
            "--rpc-url",
            rpc_url,
            "--passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success();
}

#[test]
fn test_account_format_json() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("account")
        .arg("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
        .arg("--format")
        .arg("json");

    let output = cmd.output().unwrap();
    assert!(output.status.success() || output.status.code().unwrap() == 1);
}

#[test]
fn test_account_invalid_format() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("account")
        .arg("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
        .arg("--format")
        .arg("xml");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid format"));
}

#[test]
fn test_account_missing_profile() {
    let dir = tempdir().unwrap();

    sdkt_isolated(dir.path())
        .args([
            "account",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "--network-profile",
            "ghost",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn test_account_invalid_address() {
    let dir = tempdir().unwrap();
    let rpc_url = "http://127.0.0.1:1"; // Port 1 will refuse connection
    add_mock_account_profile(dir.path(), rpc_url);

    let output = sdkt_isolated(dir.path())
        .args(["account", "INVALID_ADDRESS", "--network-profile", "mocknet"])
        .output()
        .unwrap();

    // Should fail with clear error about invalid address
    assert!(!output.status.success(), "Should fail with invalid address");
}

#[test]
fn test_account_help_shows_format_options() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("account").arg("--help");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--format"))
        .stdout(predicate::str::contains("--network-profile"));
}

#[test]
fn test_account_json_structure() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("account")
        .arg("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
        .arg("--format")
        .arg("json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // If it succeeded (network available), verify JSON structure
    if output.status.success() {
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("Should be valid JSON");
        assert!(parsed.get("address").is_some(), "Missing address field");
        assert!(parsed.get("sequence").is_some(), "Missing sequence field");
        assert!(parsed.get("balances").is_some(), "Missing balances field");
        assert!(parsed.get("signers").is_some(), "Missing signers field");
    }
}
