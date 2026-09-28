use assert_cmd::Command;
use base64::Engine as _;
use predicates::prelude::*;
use serde_json::Value;

const TEST_SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const TEST_CONTRACT: &str = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

/// Build an envelope with `tx build` so the tests exercise the documented
/// build -> decode workflow rather than a hand-made fixture.
fn built_envelope() -> String {
    let out = sdkt()
        .args([
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
            "transfer",
            "--arg",
            "u32:42",
            "--arg",
            "string:hello",
            "--arg",
            "bool:true",
            "--memo-text",
            "invoice 7",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "tx build failed: {:?}", out);
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    json["envelope"].as_str().unwrap().to_string()
}

#[test]
fn tx_decode_json_round_trips_tx_build_values() {
    let envelope = built_envelope();
    let out = sdkt()
        .args(["tx", "decode", &envelope, "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "tx decode failed: {:?}", out);
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();

    assert_eq!(view["envelope_type"], "tx");
    assert_eq!(view["source"], TEST_SOURCE);
    assert_eq!(view["sequence"], 43);
    assert_eq!(view["fee"], 250);
    assert_eq!(view["memo"], r#"text:"invoice 7""#);
    let op = &view["operations"][0];
    assert_eq!(op["kind"], "InvokeContract");
    assert_eq!(op["contract"], TEST_CONTRACT);
    assert_eq!(op["function"], "transfer");
    assert_eq!(
        op["args"],
        serde_json::json!(["u32:42", r#"string:"hello""#, "bool:true"])
    );
    assert_eq!(view["signatures"], serde_json::json!([]));
    assert!(view["fee_bump"].is_null());
}

#[test]
fn tx_decode_pretty_shows_call_details() {
    let envelope = built_envelope();
    sdkt()
        .args(["tx", "decode", &envelope])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sequence:   43"))
        .stdout(predicate::str::contains("Fee:        250 stroops"))
        .stdout(predicate::str::contains("[0] InvokeContract"))
        .stdout(predicate::str::contains("Function:  transfer"))
        .stdout(predicate::str::contains(
            r#"Args:      u32:42, string:"hello", bool:true"#,
        ))
        .stdout(predicate::str::contains("Signatures: 0"));
}

#[test]
fn tx_decode_reads_envelope_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tx.xdr");
    std::fs::write(&path, format!("{}\n", built_envelope())).unwrap();
    sdkt()
        .args(["tx", "decode", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Function:  transfer"));
}

#[test]
fn tx_decode_rejects_invalid_input() {
    sdkt()
        .args(["tx", "decode", "AAAA-not-an-envelope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid transaction envelope"));
}

#[test]
fn tx_decode_rejects_trailing_bytes_like_tx_validate() {
    let mut raw = base64::engine::general_purpose::STANDARD
        .decode(built_envelope())
        .unwrap();
    raw.extend_from_slice(&[0, 0, 0, 0]);
    let tampered = base64::engine::general_purpose::STANDARD.encode(raw);

    sdkt()
        .args(["tx", "decode", &tampered])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid transaction envelope"));
    sdkt()
        .args(["tx", "validate", "--envelope", &tampered])
        .assert()
        .failure();
}
