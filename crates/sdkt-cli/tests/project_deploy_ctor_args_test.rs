use assert_cmd::Command;
use base64::Engine;
use predicates::prelude::*;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use stellar_xdr::{
    HostFunction, Limits, OperationBody, ReadXdr, ScVal, TransactionEnvelope,
};

const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";

const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").expect("sdkt binary built")
}

fn sdkt_isolated(dir: &Path) -> Command {
    let mut cmd = sdkt();
    cmd.current_dir(dir);
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd
}

fn fixture_wasm_bytes() -> Vec<u8> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_path = manifest_dir.join("tests/fixtures/us_new.wasm");
    fs::read(&fixture_path).expect("fixture wasm exists")
}

fn setup_project(
    dir: &Path,
    sdkt_toml: &str,
    wasm_bytes: &[u8],
) {
    fs::create_dir_all(dir.join("contracts/token/target/wasm32-unknown-unknown/release")).unwrap();
    fs::write(
        dir.join("contracts/token/target/wasm32-unknown-unknown/release/token.wasm"),
        wasm_bytes,
    )
    .unwrap();
    fs::write(dir.join(".sdkt.toml"), sdkt_toml).unwrap();
}

fn mock_deploy_rpc() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let submitted_txs = Arc::new(Mutex::new(Vec::new()));
    let submitted_txs_clone = submitted_txs.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 32768];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            let body_start = req.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
            let body_str = &req[body_start..];
            let json_req: serde_json::Value =
                serde_json::from_str(body_str).unwrap_or(serde_json::Value::Null);
            let method = json_req.get("method").and_then(|m| m.as_str()).unwrap_or("");

            if method == "sendTransaction" {
                if let Some(tx_b64) = json_req["params"]["transaction"].as_str() {
                    submitted_txs_clone.lock().unwrap().push(tx_b64.to_string());
                }
            }

            let resp_body = match method {
                "getLedgerEntries" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                ),
                "simulateTransaction" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                ),
                "sendTransaction" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeef1234","status":"PENDING","latestLedger":"100"}}"#.to_string()
                }
                "getTransaction" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg=="}}"#.to_string()
                }
                _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string(),
            };

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });

    (url, submitted_txs)
}

#[test]
fn project_deploy_bad_arg_fails_fast_before_upload() {
    let tmp = tempfile::tempdir().unwrap();
    let wasm = fixture_wasm_bytes();
    let toml = r#"
[network]
default = "testnet"
rpc_url = "https://soroban-testnet.stellar.org"
passphrase = "Test SDF Network ; September 2015"

[contracts.token]
path = "contracts/token"
ctor_args = ["u32:not_a_number"]
"#;
    setup_project(tmp.path(), toml, &wasm);

    sdkt_isolated(tmp.path())
        .args(["project", "deploy"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid u32 value: not_a_number"));
}

#[test]
fn project_deploy_bad_hex_arg_fails_fast_before_upload() {
    let tmp = tempfile::tempdir().unwrap();
    let wasm = fixture_wasm_bytes();
    let toml = r#"
[network]
default = "testnet"
rpc_url = "https://soroban-testnet.stellar.org"
passphrase = "Test SDF Network ; September 2015"

[contracts.token]
path = "contracts/token"
ctor_args = ["bytes:invalid_hex!"]
"#;
    setup_project(tmp.path(), toml, &wasm);

    sdkt_isolated(tmp.path())
        .args(["project", "deploy"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid hex byte"));
}

#[test]
fn project_deploy_forwards_constructor_args_mock_rpc() {
    let tmp = tempfile::tempdir().unwrap();
    let (rpc_url, submitted_txs) = mock_deploy_rpc();
    let wasm = fixture_wasm_bytes();

    let toml = format!(
        r#"
[network]
default = "mocknet"
rpc_url = "{rpc_url}"
passphrase = "Test SDF Network ; September 2015"

[contracts.token]
path = "contracts/token"
ctor_args = ["u32:42", "bool:true"]
"#
    );
    setup_project(tmp.path(), &toml, &wasm);

    // Generate and set default identity
    sdkt_isolated(tmp.path())
        .args(["identity", "generate", "alice"])
        .assert()
        .success();
    sdkt_isolated(tmp.path())
        .args(["identity", "default", "alice"])
        .assert()
        .success();

    // Run project deploy
    sdkt_isolated(tmp.path())
        .args(["project", "deploy"])
        .assert()
        .success();

    // Verify submitted transactions
    let txs = submitted_txs.lock().unwrap().clone();
    assert!(!txs.is_empty(), "expected submitted transactions");

    // The create_contract transaction is submitted in the second phase
    let create_tx_b64 = txs.last().unwrap();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(create_tx_b64)
        .expect("valid base64 transaction");
    let TransactionEnvelope::Tx(v1) =
        TransactionEnvelope::from_xdr(raw, Limits::none()).expect("valid envelope")
    else {
        panic!("expected TransactionEnvelope::Tx");
    };

    let op = &v1.tx.operations[0].body;
    match op {
        OperationBody::InvokeHostFunction(invoke) => match &invoke.host_function {
            HostFunction::CreateContractV2(v2) => {
                assert_eq!(v2.constructor_args.len(), 2);
                assert_eq!(v2.constructor_args[0], ScVal::U32(42));
                assert_eq!(v2.constructor_args[1], ScVal::Bool(true));
            }
            other => panic!("expected CreateContractV2 host function, got: {other:?}"),
        },
        other => panic!("expected InvokeHostFunction operation, got: {other:?}"),
    }
}

#[test]
fn project_deploy_empty_ctor_args_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let (rpc_url, submitted_txs) = mock_deploy_rpc();
    let wasm = fixture_wasm_bytes();

    let toml = format!(
        r#"
[network]
default = "mocknet"
rpc_url = "{rpc_url}"
passphrase = "Test SDF Network ; September 2015"

[contracts.token]
path = "contracts/token"
"#
    );
    setup_project(tmp.path(), &toml, &wasm);

    // Generate and set default identity
    sdkt_isolated(tmp.path())
        .args(["identity", "generate", "alice"])
        .assert()
        .success();
    sdkt_isolated(tmp.path())
        .args(["identity", "default", "alice"])
        .assert()
        .success();

    // Run project deploy
    sdkt_isolated(tmp.path())
        .args(["project", "deploy"])
        .assert()
        .success();

    // Verify submitted transactions
    let txs = submitted_txs.lock().unwrap().clone();
    assert!(!txs.is_empty(), "expected submitted transactions");

    let create_tx_b64 = txs.last().unwrap();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(create_tx_b64)
        .expect("valid base64 transaction");
    let TransactionEnvelope::Tx(v1) =
        TransactionEnvelope::from_xdr(raw, Limits::none()).expect("valid envelope")
    else {
        panic!("expected TransactionEnvelope::Tx");
    };

    let op = &v1.tx.operations[0].body;
    match op {
        OperationBody::InvokeHostFunction(invoke) => match &invoke.host_function {
            HostFunction::CreateContract(_) => {
                // Byte-identical standard create contract without V2/constructor args
            }
            other => panic!("expected standard CreateContract, got: {other:?}"),
        },
        other => panic!("expected InvokeHostFunction operation, got: {other:?}"),
    }
}
