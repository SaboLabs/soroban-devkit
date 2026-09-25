use assert_cmd::Command;
use base64::Engine as _;
use predicates::prelude::*;
use serde_json::json;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use stellar_xdr::{
    Limits, Operation, OperationResult, TransactionEnvelope, TransactionResult,
    TransactionResultExt, TransactionResultResult, TransactionV1Envelope, WriteXdr,
};
use tempfile::tempdir;

fn sdkt_tx_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

fn add_mock_tx_profile(dir: &std::path::Path, rpc_url: &str) {
    sdkt_tx_isolated(dir)
        .args([
            "network",
            "add",
            "mocknet",
            "--rpc-url",
            rpc_url,
            "--passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success();
}

/// Mock RPC server that returns a simulation response with a ScVal result.
fn mock_simulate_server(scval_b64: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let result = scval_b64.to_string();

    thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf);
            let body = format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"results":[{{"xdr":"{result}","auth":[]}}],"latestLedger":"12345","events":[],"cost":{{"cpuInsns":"1000","memBytes":"2000"}}}}}}"#
            );
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(header.as_bytes());
        }
    });

    thread::sleep(Duration::from_millis(50));
    url
}

fn mock_inspect_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let result = TransactionResult {
        fee_charged: 12_500,
        result: TransactionResultResult::TxSuccess(
            vec![OperationResult::default(), OperationResult::default()]
                .try_into()
                .unwrap(),
        ),
        ext: TransactionResultExt::V0,
    };
    let mut envelope = TransactionV1Envelope::default();
    envelope.tx.operations = vec![Operation::default(), Operation::default()]
        .try_into()
        .unwrap();
    let encode = |value: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(value);
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "status": "SUCCESS",
            "ledger": 12_345,
            "resultXdr": encode(result.to_xdr(Limits::none()).unwrap()),
            "envelopeXdr": encode(TransactionEnvelope::Tx(envelope).to_xdr(Limits::none()).unwrap()),
        }
    })
    .to_string();

    thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut request = [0u8; 8192];
        let _ = sock.read(&mut request);
        write!(
            sock,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    url
}

/// Path to a real contractspecv0 WASM fixture with functions that return u32.
static WASM_WITH_RETURN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../sdkt-cli/tests/fixtures/us_new.wasm"
);

/// ScVal U32(42) in base64 XDR.
const MOCK_SCVAL_U32_42: &str = "AAAAAwAAACo=";

#[test]
fn test_tx_inspect_format_json() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("tx")
        .arg("inspect")
        .arg("0000000000000000000000000000000000000000000000000000000000000000")
        .arg("--format")
        .arg("json");

    let output = cmd.output().unwrap();
    assert!(output.status.success() || output.status.code().unwrap() == 1);
}

#[test]
fn test_tx_inspect_invalid_format() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("tx")
        .arg("inspect")
        .arg("0000000000000000000000000000000000000000000000000000000000000000")
        .arg("--format")
        .arg("xml");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid format"));
}

#[test]
fn test_tx_inspect_pretty_reports_settled_fee_and_operations() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_inspect_server();
    add_mock_tx_profile(dir.path(), &rpc_url);

    sdkt_tx_isolated(dir.path())
        .args(["tx", "--network-profile", "mocknet", "inspect", "abc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status: SUCCESS"))
        .stdout(predicate::str::contains("Fee: 12500 stroops"))
        .stdout(predicate::str::contains("Operations: 2"));
}

#[test]
fn test_tx_inspect_json_reports_settled_fee_and_operations() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_inspect_server();
    add_mock_tx_profile(dir.path(), &rpc_url);

    let output = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "--network-profile",
            "mocknet",
            "inspect",
            "abc",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["fee_charged"], 12_500);
    assert_eq!(parsed["operation_count"], 2);
    assert_eq!(parsed["ledger"], 12_345);
}

#[test]
fn test_tx_simulate_help_shows_abi_flag() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("tx").arg("simulate").arg("--help");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--abi"))
        .stdout(predicate::str::contains("--envelope"));
}

#[test]
fn test_tx_simulate_with_abi_decodes_result() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_simulate_server(MOCK_SCVAL_U32_42);
    add_mock_tx_profile(dir.path(), &rpc_url);

    let envelope = "AAAAAQ==";

    sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "--network-profile",
            "mocknet",
            "simulate",
            "--envelope",
            envelope,
            "--abi",
            WASM_WITH_RETURN,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("u32(42)"));
}

#[test]
fn test_tx_simulate_with_abi_json_output() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_simulate_server(MOCK_SCVAL_U32_42);
    add_mock_tx_profile(dir.path(), &rpc_url);

    let envelope = "AAAAAQ==";

    let output = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "--network-profile",
            "mocknet",
            "simulate",
            "--envelope",
            envelope,
            "--abi",
            WASM_WITH_RETURN,
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nOutput: {stdout}"));

    assert!(
        parsed.get("decodedResult").is_some(),
        "Missing decodedResult field"
    );
    assert_eq!(parsed["decodedResult"]["label"], "u32(42)");
}

#[test]
fn test_tx_simulate_without_abi_preserves_behavior() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_simulate_server(MOCK_SCVAL_U32_42);
    add_mock_tx_profile(dir.path(), &rpc_url);

    let envelope = "AAAAAQ==";

    sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "--network-profile",
            "mocknet",
            "simulate",
            "--envelope",
            envelope,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status: SUCCESS"))
        .stdout(predicate::str::contains("Operations: 1 results"));
}

#[test]
fn test_tx_simulate_with_invalid_abi_fails() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_simulate_server(MOCK_SCVAL_U32_42);
    add_mock_tx_profile(dir.path(), &rpc_url);

    let invalid_wasm = dir.path().join("invalid.wasm");
    std::fs::write(&invalid_wasm, b"not a wasm file").unwrap();

    let envelope = "AAAAAQ==";

    sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "--network-profile",
            "mocknet",
            "simulate",
            "--envelope",
            envelope,
            "--abi",
            invalid_wasm.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Failed to parse ABI"));
}
