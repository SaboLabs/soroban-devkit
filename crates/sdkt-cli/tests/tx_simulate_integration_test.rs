use assert_cmd::Command;

#[test]
fn test_cli_tx_simulate_empty_envelope() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("simulate")
        .arg("--envelope")
        .arg("   ")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("Transaction envelope is empty"));
}

#[test]
fn test_cli_tx_simulate_invalid_envelope() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("simulate")
        .arg("--envelope")
        .arg("not_real_base64_or_file")
        .assert();
    // It should hit the network and the network returns an RPC error
    // "Rpc error: Transaction envelope is invalid" or similar
    assert
        .failure()
        .stderr(predicates::str::contains("Error simulating transaction"));
}
