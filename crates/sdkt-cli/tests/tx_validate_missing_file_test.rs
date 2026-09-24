use assert_cmd::Command;

#[test]
fn test_cli_tx_validate_missing_envelope_file() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("validate")
        .arg("--envelope")
        .arg("/no/such/tx-validate-envelope.xdr")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("invalid file"));
}
