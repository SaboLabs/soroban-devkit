use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_tx_inspect_format_json() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("tx")
        .arg("inspect")
        // Just checking command structure and error propagation, not a real hash if offline
        .arg("0000000000000000000000000000000000000000000000000000000000000000")
        .arg("--format")
        .arg("json");

    // We don't assert success as RPC might fail, but ensure it runs without panicking on args
    let output = cmd.output().unwrap();
    assert!(output.status.success() || output.status.code().unwrap() == 1);
}

#[test]
fn test_tx_inspect_invalid_format() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("tx")
        .arg("inspect")
        .arg("0000000000000000000000000000000000000000000000000000000000000000")
        .arg("--format")
        .arg("xml");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid format"));
}
