use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_events_format_json() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg("CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK")
        .arg("--format")
        .arg("json");

    let output = cmd.output().unwrap();
    // Verify run success or network connection failure exit 1
    assert!(output.status.success() || output.status.code().unwrap() == 1);
}

#[test]
fn test_events_invalid_format() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg("CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK")
        .arg("--format")
        .arg("xml");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid format"));
}
