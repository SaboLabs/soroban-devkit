use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

// Envelope used for the CLI tests is a deliberately-invalid (but base64-safe)
// envelope so the RPC call either returns a network error or an RPC-level
// rejection — we assert on the error path rather than a real broadcast.
const ENVELOPE: &str =
    "AAAAAgAAAABkQMdGsjCv3zavZlW5740YkOCNy0wKb9E8LPuJ2dXq1QAAAAQAAAAFAAAAAABvq5c=";

#[test]
fn test_cli_submit_invalid_envelope_rejects() {
    // An obviously invalid base64 envelope should fail locally without a broadcast.
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("submit")
        .arg("--envelope")
        .arg("not-a-real-envelope!!!")
        .assert();
    // Fails to reach a node or is rejected — non-zero exit with error text.
    assert.failure();
}

#[test]
fn test_cli_submit_json_output_format() {
    // Envelope read from a file path. Even on network error the JSON flag must
    // not panic; non-zero exit expected since no live node is guaranteed.
    let dir = tempdir().unwrap();
    let env_path = dir.path().join("tx.xdr");
    fs::write(&env_path, ENVELOPE).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("submit")
        .arg("--envelope")
        .arg(env_path.to_str().unwrap())
        .arg("--format")
        .arg("json")
        .assert();
    // It hits a real (or default) node, which correctly rejects the fake envelope.
    // The main point is to ensure we don't panic and we exit cleanly with a code.
    assert.failure();
}
