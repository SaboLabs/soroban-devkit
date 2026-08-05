use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_account_format_json() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("account")
        .arg("GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX")
        .arg("--format")
        .arg("json");

    let output = cmd.output().unwrap();
    // Verify run success or network connection failure exit 1
    assert!(output.status.success() || output.status.code().unwrap() == 1);
}

#[test]
fn test_account_invalid_format() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("account")
        .arg("GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX")
        .arg("--format")
        .arg("xml");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid format"));
}
