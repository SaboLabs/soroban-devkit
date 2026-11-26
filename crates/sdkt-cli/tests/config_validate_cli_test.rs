use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

#[test]
fn config_validate_no_config_file_exits_zero_with_note() {
    let tmp = TempDir::new().unwrap();
    sdkt()
        .current_dir(tmp.path())
        .args(["config", "validate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No .sdkt.toml found"))
        .stdout(predicate::str::contains("Configuration is valid"));
}

#[test]
fn config_validate_valid_fixture_exits_zero() {
    let tmp = TempDir::new().unwrap();
    let contract_dir = tmp.path().join("contracts/token");
    fs::create_dir_all(&contract_dir).unwrap();
    fs::write(
        contract_dir.join("Cargo.toml"),
        "[package]\nname = \"token\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join(".sdkt.toml"),
        r#"
[network]
rpc_url = "https://soroban-testnet.stellar.org"
passphrase = "Test SDF Network ; September 2015"
timeout_secs = 15

[decode]
max_depth = 0

[storage]
max_entries = 100

[package]
name = "demo"
version = "0.1.0"
custom_forward_compatible_key = "allowed"

[contracts.token]
path = "contracts/token"
"#,
    )
    .unwrap();

    sdkt()
        .current_dir(tmp.path())
        .args(["config", "validate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Configuration is valid"));

    let out = sdkt()
        .current_dir(tmp.path())
        .args(["config", "validate", "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["valid"], true);
    assert_eq!(parsed["problems"].as_array().unwrap().len(), 0);
}

#[test]
fn config_validate_unknown_keys_and_semantic_errors_exit_one() {
    let tmp = TempDir::new().unwrap();

    // Stage 1: typo'd top-level key [netwrok]
    fs::write(
        tmp.path().join(".sdkt.toml"),
        "[netwrok]\nrpc_url = \"http://localhost:8000\"\n",
    )
    .unwrap();
    sdkt()
        .current_dir(tmp.path())
        .args(["config", "validate"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("error: root.netwrok:"))
        .stderr(predicate::str::contains("1 problem(s) found"));

    // Stage 2: storage.max_entries = 0 + missing contract path (JSON output)
    fs::write(
        tmp.path().join(".sdkt.toml"),
        "[storage]\nmax_entries = 0\n[contracts.token]\npath = \"contracts/missing\"\n",
    )
    .unwrap();
    let out = sdkt()
        .current_dir(tmp.path())
        .args(["config", "validate", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["valid"], false);
    let problems = parsed["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 2);
    assert!(problems
        .iter()
        .any(|p| p["section"] == "storage" && p["key"] == "max_entries"));
    assert!(problems
        .iter()
        .any(|p| p["section"] == "contracts.token" && p["key"] == "path"));
}

#[test]
fn config_validate_resolves_relative_paths_against_config_directory() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("project");
    let other = tmp.path().join("other");
    fs::create_dir_all(proj.join("contracts/token")).unwrap();
    fs::create_dir_all(&other).unwrap();
    fs::write(
        proj.join("contracts/token/Cargo.toml"),
        "[package]\nname = \"token\"\n",
    )
    .unwrap();
    let toml_text = "[contracts.token]\npath = \"contracts/token\"\n";
    fs::write(proj.join(".sdkt.toml"), toml_text).unwrap();
    fs::write(other.join(".sdkt.toml"), toml_text).unwrap();

    // Valid when run from project directory
    sdkt()
        .current_dir(&proj)
        .args(["config", "validate"])
        .assert()
        .success();

    // Fails when run from another directory where contracts/token does not exist
    sdkt()
        .current_dir(&other)
        .args(["config", "validate"])
        .assert()
        .failure()
        .code(1);
}
