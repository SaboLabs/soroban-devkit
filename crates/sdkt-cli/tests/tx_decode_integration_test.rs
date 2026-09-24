//! CLI tests for `sdkt tx decode` (human-readable envelope viewer).

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::NamedTempFile;

const TEST_SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const TEST_CONTRACT: &str = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";

fn build_envelope(extra_args: &[&str]) -> String {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let mut args = vec![
        "tx",
        "build",
        "--source",
        TEST_SOURCE,
        "--sequence",
        "43",
        "--fee",
        "250",
        "--contract",
        TEST_CONTRACT,
        "--function",
        "increment",
        "--format",
        "json",
    ];
    args.extend_from_slice(extra_args);
    let output = cmd.args(args).output().unwrap();
    assert!(
        output.status.success(),
        "tx build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // {"envelope": "AAAA..."}
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    json["envelope"].as_str().unwrap().to_string()
}

#[test]
fn test_tx_decode_pretty_shows_core_fields() {
    let envelope = build_envelope(&["--arg", "u32:42"]);

    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["tx", "decode", &envelope])
        .assert()
        .success()
        .stdout(predicate::str::contains("Transaction Envelope:"))
        .stdout(predicate::str::contains(TEST_SOURCE))
        .stdout(predicate::str::contains("Sequence:   43"))
        .stdout(predicate::str::contains("Fee:        250 stroops"))
        .stdout(predicate::str::contains("InvokeContract"))
        .stdout(predicate::str::contains("increment"))
        .stdout(predicate::str::contains("u32:42"))
        .stdout(predicate::str::contains(TEST_CONTRACT));
}

#[test]
fn test_tx_decode_json_structured() {
    let envelope = build_envelope(&["--arg", "u32:42"]);

    let output = Command::cargo_bin("sdkt")
        .unwrap()
        .args(["tx", "decode", &envelope, "--format", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(json["type"], "tx");
    assert_eq!(json["sequence"], 43);
    assert_eq!(json["fee"], 250);
    assert_eq!(json["source"], TEST_SOURCE);
    assert_eq!(json["operations"][0]["type"], "InvokeContract");
    assert_eq!(json["operations"][0]["function"], "increment");
    assert_eq!(json["operations"][0]["args"][0], "u32:42");
    // Must be structured view, not raw XDR serde
    assert!(json.get("Tx").is_none());
}

#[test]
fn test_tx_decode_from_file() {
    let envelope = build_envelope(&[]);
    let mut temp = NamedTempFile::new().unwrap();
    use std::io::Write;
    writeln!(temp, "{}", envelope).unwrap();

    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["tx", "decode", temp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Operations (1):"));
}

#[test]
fn test_tx_decode_invalid_exits_nonzero() {
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["tx", "decode", "not-valid-xdr!!!"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"));
}

#[test]
fn test_tx_validate_unchanged_regression() {
    let envelope = build_envelope(&[]);
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["tx", "validate", "--envelope", &envelope])
        .assert()
        .success()
        .stdout(predicate::str::contains("VALID"));
}
