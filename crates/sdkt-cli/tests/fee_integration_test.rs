use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_cli_fee_estimate_testnet() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("fee")
        .arg("estimate")
        .arg("--network")
        .arg("testnet")
        .arg("--base-fees")
        .arg("100,120,90,110")
        .arg("--format")
        .arg("pretty");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Fee Estimate (testnet):"))
        .stdout(predicate::str::contains("Stroops: 100"))
        .stdout(predicate::str::contains("XLM: 0.00001"));
}

#[test]
fn test_cli_fee_estimate_mainnet_json() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("fee")
        .arg("estimate")
        .arg("--network")
        .arg("mainnet")
        .arg("--base-fees")
        .arg("100")
        .arg("--format")
        .arg("json");

    cmd.assert().success().stdout(predicate::str::contains(
        r#"{"stroops":125,"xlm":"0.0000125"}"#,
    ));
}

#[test]
fn test_cli_fee_estimate_invalid_fees() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("fee")
        .arg("estimate")
        .arg("--base-fees")
        .arg("100,abc,120");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid base_fees"));
}

#[test]
fn test_cli_fee_estimate_unknown_network() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("fee")
        .arg("estimate")
        .arg("--network")
        .arg("fakenet")
        .arg("--base-fees")
        .arg("100");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid network"));
}
