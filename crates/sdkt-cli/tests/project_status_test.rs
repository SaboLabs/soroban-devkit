//! Integration tests for read-only `sdkt project status` output and scope.

use assert_cmd::Command;
use sdkt_core::deployment::{DeploymentRecord, DeploymentRecordFile, DEPLOYMENT_RECORD_FILE};
use std::path::Path;
use tempfile::TempDir;

fn sdkt(project: &Path, network_store: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(project)
        .env("SDKT_NETWORK_DIR", network_store)
        .env("SDKT_IDENTITY_DIR", project.join("identity"));
    cmd
}

fn record(alias: &str, contract_id: &str, network: &str) -> DeploymentRecord {
    DeploymentRecord {
        contract_id: contract_id.to_string(),
        wasm_hash: format!("hash-{alias}"),
        network: network.to_string(),
        timestamp: 1_700_000_000,
        salt: Some(format!("salt-{alias}")),
    }
}

fn write_records(project: &Path, entries: &[(&str, &str, &str)]) {
    let mut file = DeploymentRecordFile::default();
    for (scope, alias, contract_id) in entries {
        file.set_record(scope, alias, record(alias, contract_id, scope));
    }
    file.write(project.join(DEPLOYMENT_RECORD_FILE))
        .expect("write deployment records");
}

#[test]
fn status_pretty_shows_record_details_without_network_or_mutation() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();
    write_records(
        project.path(),
        &[
            ("testnet", "token", "CTOKEN"),
            ("mainnet", "token", "CMAIN"),
        ],
    );
    let path = project.path().join(DEPLOYMENT_RECORD_FILE);
    let before = std::fs::read(&path).unwrap();

    sdkt(project.path(), network_store.path())
        .args(["project", "status", "--rpc-url", "http://127.0.0.1:1"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Project deployment status (network: testnet):",
        ))
        .stdout(predicates::str::contains("token"))
        .stdout(predicates::str::contains("Contract ID: CTOKEN"))
        .stdout(predicates::str::contains("WASM hash:   hash-token"))
        .stdout(predicates::str::contains("Recorded network: testnet"))
        .stdout(predicates::str::contains("Timestamp:  1700000000"))
        .stdout(predicates::str::contains("Salt:       salt-token"));

    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(!project.path().join(".sdkt-deployments.tmp").exists());
}

#[test]
fn status_json_is_stable_and_machine_readable() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();
    write_records(
        project.path(),
        &[("testnet", "zeta", "CZETA"), ("testnet", "alpha", "CALPHA")],
    );

    let output = sdkt(project.path(), network_store.path())
        .args(["project", "status", "--format", "json"])
        .output()
        .expect("status command runs");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["status"], "deployed");
    assert_eq!(parsed["network"], "testnet");
    let deployments = parsed["deployments"].as_array().unwrap();
    assert_eq!(deployments.len(), 2);
    assert_eq!(deployments[0]["alias"], "alpha");
    assert_eq!(deployments[0]["contract_id"], "CALPHA");
    assert_eq!(deployments[0]["wasm_hash"], "hash-alpha");
    assert_eq!(deployments[0]["timestamp"], 1_700_000_000);
    assert_eq!(deployments[0]["salt"], "salt-alpha");
    assert_eq!(deployments[1]["alias"], "zeta");
}

#[test]
fn status_succeeds_for_missing_and_empty_records() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    let missing = sdkt(project.path(), network_store.path())
        .args(["project", "status", "--format", "json"])
        .output()
        .expect("status command runs");
    assert!(missing.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(parsed["status"], "no_deployments");
    assert_eq!(parsed["deployments"], serde_json::json!([]));

    let path = project.path().join(DEPLOYMENT_RECORD_FILE);
    std::fs::write(&path, "  \n\t").unwrap();
    let empty_contents = std::fs::read(&path).unwrap();
    sdkt(project.path(), network_store.path())
        .args(["project", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No deployments recorded"));
    assert_eq!(std::fs::read(path).unwrap(), empty_contents);
}

#[test]
fn status_fails_clearly_on_corrupt_record_without_mutation() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();
    let path = project.path().join(DEPLOYMENT_RECORD_FILE);
    std::fs::write(&path, "not json {").unwrap();
    let before = std::fs::read(&path).unwrap();

    sdkt(project.path(), network_store.path())
        .args(["project", "status"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "Failed to parse .sdkt-deployments.json",
        ));

    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn status_uses_profile_and_passphrase_scope_in_current_project() {
    let project = TempDir::new().unwrap();
    let other_project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    sdkt(project.path(), network_store.path())
        .args([
            "network",
            "add",
            "qa",
            "--rpc-url",
            "http://127.0.0.1:1",
            "--passphrase",
            "Public Global Stellar Network ; September 2015",
        ])
        .assert()
        .success();

    write_records(
        project.path(),
        &[("qa", "selected", "CQA"), ("testnet", "override", "CTEST")],
    );
    write_records(other_project.path(), &[("qa", "selected", "COTHER")]);

    let profile = sdkt(project.path(), network_store.path())
        .args([
            "project",
            "status",
            "--network-profile",
            "qa",
            "--format",
            "json",
        ])
        .output()
        .expect("profile-scoped status runs");
    assert!(profile.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&profile.stdout).unwrap();
    assert_eq!(parsed["network"], "qa");
    assert_eq!(parsed["deployments"][0]["contract_id"], "CQA");

    let passphrase_output = sdkt(project.path(), network_store.path())
        .args([
            "project",
            "status",
            "--network-profile",
            "qa",
            "--network-passphrase",
            "Test SDF Network ; September 2015",
            "--format",
            "json",
        ])
        .output()
        .expect("explicit-passphrase-scoped status runs");
    assert!(passphrase_output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&passphrase_output.stdout).unwrap();
    assert_eq!(parsed["network"], "testnet");
    assert_eq!(parsed["deployments"][0]["contract_id"], "CTEST");
}
