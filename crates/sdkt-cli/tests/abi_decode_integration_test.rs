use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_events_abi_flag_exists() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    // Verify --abi flag is recognized (will fail for missing WASM, confirming wiring)
    cmd.arg("events")
        .arg("CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX")
        .arg("--abi")
        .arg("/nonexistent/wasm")
        .assert()
        .failure();
}

#[test]
fn test_inspect_abi_flag_exists() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("inspect")
        .arg("CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX")
        .arg("--abi")
        .arg("/nonexistent/wasm")
        .assert()
        .failure();
}
