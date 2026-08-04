use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_decode_scval_integer() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    // AAAAAgAAAAk= is base64 for ScVal (U32 9) but SeVal XDR parsing may fail if truncated.
    // We will use an explicitly generated correct XDR base64 for ScVal_I32(42) instead.
    // A quick valid test can be done for TransactionResult or LedgerEntry
    // ScVal I32(1) = "AAAAAwAAAAE="
    cmd.arg("decode")
        .arg("AAAAAwAAAAE=")
        .arg("--type")
        .arg("ScVal")
        .arg("--format")
        .arg("json");

    cmd.assert().success();
}

#[test]
fn test_inspect_format_flag() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("inspect")
        .arg("CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK")
        .arg("--format")
        .arg("invalidformat");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid format"));
}
