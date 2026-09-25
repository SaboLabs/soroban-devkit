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
    FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionExt,
    FeeBumpTransactionInnerTx, Limits, MuxedAccount, Operation, OperationResult,
    TransactionEnvelope, TransactionResult, TransactionResultExt, TransactionResultResult,
    TransactionV1Envelope, Uint256, VecM, WriteXdr,
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

fn mock_submit_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "status": "PENDING",
            "hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "latestLedger": 12_345,
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

fn mock_inspect_fee_bump_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let result = TransactionResult {
        fee_charged: 20_000,
        result: TransactionResultResult::TxSuccess(
            vec![OperationResult::default(), OperationResult::default()]
                .try_into()
                .unwrap(),
        ),
        ext: TransactionResultExt::V0,
    };
    let mut inner = TransactionV1Envelope::default();
    inner.tx.operations = vec![Operation::default(), Operation::default()]
        .try_into()
        .unwrap();
    let fee_bump = FeeBumpTransactionEnvelope {
        tx: FeeBumpTransaction {
            fee_source: MuxedAccount::Ed25519(Uint256([1u8; 32])),
            fee: 20_000,
            inner_tx: FeeBumpTransactionInnerTx::Tx(inner),
            ext: FeeBumpTransactionExt::V0,
        },
        signatures: VecM::default(),
    };
    let encode = |value: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(value);
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "status": "SUCCESS",
            "ledger": 12_345,
            "resultXdr": encode(result.to_xdr(Limits::none()).unwrap()),
            "envelopeXdr": encode(TransactionEnvelope::TxFeeBump(fee_bump).to_xdr(Limits::none()).unwrap()),
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

#[test]
fn test_tx_wrap_help_shows_required_flags() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("tx").arg("wrap").arg("--help");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--envelope"))
        .stdout(predicate::str::contains("--fee-source"))
        .stdout(predicate::str::contains("--fee"));
}

#[test]
fn test_tx_wrap_pretty_and_json_output() {
    let dir = tempdir().unwrap();
    let build_out = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "build",
            "--source",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "--sequence",
            "123",
            "--contract",
            "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
            "--function",
            "hello",
            "--fee",
            "100",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(build_out.status.success());
    let build_json: serde_json::Value = serde_json::from_slice(&build_out.stdout).unwrap();
    let inner_envelope = build_json["envelope"].as_str().unwrap();

    // 1. Pretty output test
    let pretty_out = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "wrap",
            "--envelope",
            inner_envelope,
            "--fee-source",
            "GBRPYHIL2CI3FNQ4BXLFMNDLFJUNPU2HY3ZMFSHONUCEOASW7QC7OX2H",
            "--fee",
            "200",
        ])
        .output()
        .unwrap();
    assert!(pretty_out.status.success());
    let pretty_stdout = String::from_utf8_lossy(&pretty_out.stdout);
    assert!(pretty_stdout.contains("Fee-Bump Transaction:"));
    assert!(pretty_stdout
        .contains("Fee Source:    GBRPYHIL2CI3FNQ4BXLFMNDLFJUNPU2HY3ZMFSHONUCEOASW7QC7OX2H"));
    assert!(pretty_stdout.contains("Inner Fee:     100 stroops"));
    assert!(pretty_stdout.contains("Fee-Bump Fee:  200 stroops"));
    assert!(pretty_stdout.contains("Envelope (Base64):"));

    // 2. JSON output test
    let json_out = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "wrap",
            "--envelope",
            inner_envelope,
            "--fee-source",
            "GBRPYHIL2CI3FNQ4BXLFMNDLFJUNPU2HY3ZMFSHONUCEOASW7QC7OX2H",
            "--fee",
            "200",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(json_out.status.success());
    let wrap_json: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert!(wrap_json["envelope"].is_string());
    assert_eq!(wrap_json["inner_fee"], 100);
    assert_eq!(wrap_json["fee"], 200);
    assert_eq!(wrap_json["fee_bump_fee"], 200);
    assert_eq!(
        wrap_json["fee_source"],
        "GBRPYHIL2CI3FNQ4BXLFMNDLFJUNPU2HY3ZMFSHONUCEOASW7QC7OX2H"
    );
    assert_eq!(wrap_json["inner_operations"], 1);
    assert_eq!(wrap_json["effective_operations"], 2);
}

#[test]
fn test_tx_wrap_envelope_from_file_and_to_file() {
    let dir = tempdir().unwrap();
    let env_file = dir.path().join("inner.xdr");
    let out_file = dir.path().join("wrapped.xdr");

    sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "build",
            "--source",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "--sequence",
            "123",
            "--contract",
            "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
            "--function",
            "hello",
            "--fee",
            "100",
            "--output",
            env_file.to_str().unwrap(),
        ])
        .assert()
        .success();

    sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "wrap",
            "--envelope",
            env_file.to_str().unwrap(),
            "--fee-source",
            "GBRPYHIL2CI3FNQ4BXLFMNDLFJUNPU2HY3ZMFSHONUCEOASW7QC7OX2H",
            "--fee",
            "250",
            "--output",
            out_file.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("written to"));

    let content = std::fs::read_to_string(&out_file).unwrap();
    assert!(!content.trim().is_empty());
}

#[test]
fn test_tx_wrap_fee_below_minimum_rejected() {
    let dir = tempdir().unwrap();
    let build_out = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "build",
            "--source",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "--sequence",
            "123",
            "--contract",
            "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
            "--function",
            "hello",
            "--fee",
            "100",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let build_json: serde_json::Value = serde_json::from_slice(&build_out.stdout).unwrap();
    let inner_envelope = build_json["envelope"].as_str().unwrap();

    sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "wrap",
            "--envelope",
            inner_envelope,
            "--fee-source",
            "GBRPYHIL2CI3FNQ4BXLFMNDLFJUNPU2HY3ZMFSHONUCEOASW7QC7OX2H",
            "--fee",
            "150",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("below required minimum of 200"));
}

#[test]
fn test_tx_wrap_pipeline_feeds_validate_sign_and_submit() {
    let dir = tempdir().unwrap();

    // 1. Generate alice and bob identities
    sdkt_tx_isolated(dir.path())
        .args(["identity", "generate", "alice"])
        .assert()
        .success();
    sdkt_tx_isolated(dir.path())
        .args(["identity", "generate", "bob"])
        .assert()
        .success();

    // 2. Build unsigned transaction for alice
    let build_out = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "build",
            "--source",
            "alice",
            "--sequence",
            "10",
            "--contract",
            "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
            "--function",
            "hello",
            "--fee",
            "100",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(build_out.status.success());
    let build_json: serde_json::Value = serde_json::from_slice(&build_out.stdout).unwrap();
    let unsigned_env = build_json["envelope"].as_str().unwrap();

    // 3. Alice signs the inner transaction
    let sign_inner_out = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "sign",
            "--input",
            unsigned_env,
            "--identity",
            "alice",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(sign_inner_out.status.success());
    let sign_inner_json: serde_json::Value =
        serde_json::from_slice(&sign_inner_out.stdout).unwrap();
    let signed_inner_env = sign_inner_json["envelope"].as_str().unwrap();

    // 4. Wrap with bob as fee source (referencing bob by identity name!)
    let wrap_out = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "wrap",
            "--envelope",
            signed_inner_env,
            "--fee-source",
            "bob",
            "--fee",
            "200",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        wrap_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&wrap_out.stderr)
    );
    let wrap_json: serde_json::Value = serde_json::from_slice(&wrap_out.stdout).unwrap();
    let wrapped_env = wrap_json["envelope"].as_str().unwrap();

    // 5. Validate the wrapped envelope using `sdkt tx validate`
    sdkt_tx_isolated(dir.path())
        .args(["tx", "validate", "--envelope", wrapped_env])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status: VALID"));

    // 6. Bob signs the fee-bump layer using `sdkt tx sign`
    let sign_bump_out = sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "sign",
            "--input",
            wrapped_env,
            "--identity",
            "bob",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(sign_bump_out.status.success());
    let sign_bump_json: serde_json::Value = serde_json::from_slice(&sign_bump_out.stdout).unwrap();
    let signed_bump_env = sign_bump_json["envelope"].as_str().unwrap();

    // 7. Validate the fully signed envelope
    sdkt_tx_isolated(dir.path())
        .args(["tx", "validate", "--envelope", signed_bump_env])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status: VALID"));

    // 8. Submit to mock RPC server
    let rpc_url = mock_submit_server();
    add_mock_tx_profile(dir.path(), &rpc_url);

    sdkt_tx_isolated(dir.path())
        .args([
            "tx",
            "--network-profile",
            "mocknet",
            "submit",
            "--envelope",
            signed_bump_env,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status: Pending"));
}

#[test]
fn test_tx_inspect_reports_wrapped_envelope() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_inspect_fee_bump_server();
    add_mock_tx_profile(dir.path(), &rpc_url);

    sdkt_tx_isolated(dir.path())
        .args(["tx", "--network-profile", "mocknet", "inspect", "abc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status: SUCCESS"))
        .stdout(predicate::str::contains("Fee: 20000 stroops"))
        .stdout(predicate::str::contains("Operations: 2"));
}
