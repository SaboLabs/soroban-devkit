//! Integration tests for `sdkt run` — the checked-in multi-step scenario
//! runner (#549).
//!
//! All network interaction is served by an in-process mock JSON-RPC server, so
//! these tests are CI-safe and never touch a live network.

use assert_cmd::Command;
use base64::Engine as _;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use stellar_xdr::{
    ContractEvent, Int128Parts, Limits, ReadXdr, ScVal, SorobanTransactionMeta,
    TransactionEnvelope, TransactionMeta, TransactionMetaV3, WriteXdr,
};
use tempfile::tempdir;

fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

const VALID_CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const VALID_ADDRESS: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

/// LedgerEntry XDR: an account entry for GAAA...WHF, balance 10 XLM, seq 41.
const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";

/// SorobanTransactionData XDR: empty footprint, 1000 instructions, 150 stroops.
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

fn i128_scval_base64(value: i64) -> String {
    let scval = ScVal::I128(Int128Parts {
        hi: 0,
        lo: value as u64,
    });
    base64::engine::general_purpose::STANDARD.encode(scval.to_xdr(Limits::none()).unwrap())
}

fn result_meta_xdr() -> String {
    let event = ContractEvent::default();
    let meta = TransactionMeta::V3(TransactionMetaV3 {
        soroban_meta: Some(SorobanTransactionMeta {
            events: vec![event].try_into().unwrap(),
            ..Default::default()
        }),
        ..Default::default()
    });
    base64::engine::general_purpose::STANDARD.encode(meta.to_xdr(Limits::none()).unwrap())
}

/// Request log: JSON-RPC method names plus the raw bodies of `sendTransaction`
/// calls, so tests can prove what was (or was not) submitted.
#[derive(Default)]
struct Seen {
    methods: Vec<String>,
    submits: Vec<String>,
}

/// Mock JSON-RPC server for the call → invoke scenario flow.
fn mock_rpc_server() -> (String, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let seen: Arc<Mutex<Seen>> = Default::default();
    let seen_thread = seen.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 32768];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let method = extract_method(&req).unwrap_or_default();

            {
                let mut log = seen_thread.lock().unwrap();
                log.methods.push(method.clone());
                if method == "sendTransaction" {
                    // Keep only the JSON-RPC body, dropping the HTTP request line
                    // and headers.
                    let body_start = req.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                    log.submits.push(req[body_start..].to_string());
                }
            }

            let body = match method.as_str() {
                "getLedgerEntries" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                ),
                "simulateTransaction" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[{{"xdr":"{}","auth":[]}}],"latestLedger":"100","events":[]}}}}"#,
                    i128_scval_base64(1000)
                ),
                "sendTransaction" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeefcafe","status":"PENDING","latestLedger":"100"}}"#.to_string()
                }
                "getTransaction" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg==","resultMetaXdr":"{}"}}}}"#,
                    result_meta_xdr()
                ),
                _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string(),
            };

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    (url, seen)
}

fn extract_method(req: &str) -> Option<String> {
    let idx = req.find("\"method\"")?;
    let rest = &req[idx + 8..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let quote = after.find('"')?;
    let end = after[quote + 1..].find('"')?;
    Some(after[quote + 1..quote + 1 + end].to_string())
}

fn add_mock_profile(dir: &std::path::Path, rpc_url: &str) {
    sdkt_isolated(dir)
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

fn generate_identity(dir: &std::path::Path, name: &str) {
    sdkt_isolated(dir)
        .args(["identity", "generate", name])
        .assert()
        .success();
}

fn write_scenario(dir: &std::path::Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn passing_scenario() -> String {
    format!(
        r#"
# Read the balance, then transfer exactly that amount using the interpolated
# result, and assert the transaction hash was recorded.
[[steps]]
id = "balance"
kind = "call"
contract = "{VALID_CONTRACT}"
function = "balance"
args = ["address:{VALID_ADDRESS}"]

[[steps.assert]]
path = "/result"
equals = 1000

[[steps]]
id = "transfer"
kind = "invoke"
contract = "{VALID_CONTRACT}"
function = "transfer"
identity = "alice"
args = ["address:{VALID_ADDRESS}", "address:{VALID_ADDRESS}", "i128:${{steps.balance.result}}"]

[[steps.assert]]
path = "/tx_hash"
exists = true
"#
    )
}

#[test]
fn run_help_documents_scenario_file_and_dry_run() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("SCENARIO.toml"))
        .stdout(predicates::str::contains("--dry-run"))
        .stdout(predicates::str::contains("--format"));
}

#[test]
fn two_step_scenario_passes_values_through_and_reports_json() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, seen) = mock_rpc_server();
    add_mock_profile(dir.path(), &url);
    let scenario = write_scenario(dir.path(), "flow.toml", &passing_scenario());

    let output = sdkt_isolated(dir.path())
        .args([
            "run",
            scenario.to_str().unwrap(),
            "--network-profile",
            "mocknet",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["status"], "passed");
    assert_eq!(summary["steps"].as_array().unwrap().len(), 2);
    assert_eq!(summary["steps"][0]["id"], "balance");
    assert_eq!(summary["steps"][0]["status"], "passed");
    assert_eq!(summary["steps"][0]["result"], 1000);
    assert_eq!(summary["steps"][1]["id"], "transfer");
    assert_eq!(summary["steps"][1]["tx_hash"], "deadbeefcafe");

    // Prove value passing reached the wire: the submitted transaction's third
    // argument is the amount produced by the call step.
    let log = seen.lock().unwrap();
    assert_eq!(log.submits.len(), 1, "expected exactly one submission");
    let request: serde_json::Value = serde_json::from_str(&log.submits[0]).unwrap();
    let envelope_b64 = request["params"]["transaction"].as_str().unwrap();
    let envelope = TransactionEnvelope::from_xdr_base64(envelope_b64, Limits::none()).unwrap();
    let TransactionEnvelope::Tx(v1) = envelope else {
        panic!("expected a v1 transaction envelope");
    };
    let stellar_xdr::OperationBody::InvokeHostFunction(op) = &v1.tx.operations[0].body else {
        panic!("expected an InvokeHostFunction operation");
    };
    let stellar_xdr::HostFunction::InvokeContract(args) = &op.host_function else {
        panic!("expected an InvokeContract host function");
    };
    assert_eq!(
        args.args.as_slice()[2],
        ScVal::I128(Int128Parts { hi: 0, lo: 1000 })
    );
}

#[test]
fn failing_assertion_exits_1_names_the_step_and_stops_the_run() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, seen) = mock_rpc_server();
    add_mock_profile(dir.path(), &url);
    let scenario = write_scenario(
        dir.path(),
        "failing.toml",
        &format!(
            r#"
[[steps]]
id = "balance"
kind = "call"
contract = "{VALID_CONTRACT}"
function = "balance"
args = ["address:{VALID_ADDRESS}"]

[[steps.assert]]
path = "/result"
equals = 999

[[steps]]
id = "transfer"
kind = "invoke"
contract = "{VALID_CONTRACT}"
function = "transfer"
identity = "alice"
args = ["address:{VALID_ADDRESS}", "address:{VALID_ADDRESS}", "i128:1"]
"#
        ),
    );

    let output = sdkt_isolated(dir.path())
        .args([
            "run",
            scenario.to_str().unwrap(),
            "--network-profile",
            "mocknet",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("step 'balance'"), "stderr: {stderr}");
    assert!(
        stderr.contains("expected 999"),
        "assertion detail missing: {stderr}"
    );
    // The run stopped before the second step.
    assert!(seen.lock().unwrap().submits.is_empty());
}

#[test]
fn unknown_step_reference_exits_2_before_any_step_runs() {
    let dir = tempdir().unwrap();
    let scenario = write_scenario(
        dir.path(),
        "unknown.toml",
        &format!(
            r#"
[[steps]]
id = "transfer"
kind = "invoke"
contract = "{VALID_CONTRACT}"
function = "transfer"
args = ["i128:${{steps.nope.result}}"]
"#
        ),
    );

    sdkt_isolated(dir.path())
        .args(["run", scenario.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unknown step 'nope'"));
}

#[test]
fn unknown_contract_alias_exits_2() {
    let dir = tempdir().unwrap();
    let scenario = write_scenario(
        dir.path(),
        "alias.toml",
        r#"
[[steps]]
id = "balance"
kind = "call"
contract = "missing-alias"
function = "balance"
"#,
    );

    sdkt_isolated(dir.path())
        .args(["run", scenario.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("alias 'missing-alias'"));
}

#[test]
fn duplicate_step_id_exits_2() {
    let dir = tempdir().unwrap();
    let scenario = write_scenario(
        dir.path(),
        "dup.toml",
        &format!(
            r#"
[[steps]]
id = "balance"
kind = "call"
contract = "{VALID_CONTRACT}"
function = "balance"

[[steps]]
id = "balance"
kind = "call"
contract = "{VALID_CONTRACT}"
function = "balance"
"#
        ),
    );

    sdkt_isolated(dir.path())
        .args(["run", scenario.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("duplicate step id 'balance'"));
}

#[test]
fn dry_run_simulates_every_step_and_submits_nothing() {
    let dir = tempdir().unwrap();
    let (url, seen) = mock_rpc_server();
    add_mock_profile(dir.path(), &url);
    let scenario = write_scenario(dir.path(), "dry.toml", &passing_scenario());

    let output = sdkt_isolated(dir.path())
        .args([
            "run",
            scenario.to_str().unwrap(),
            "--network-profile",
            "mocknet",
            "--dry-run",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["dry_run"], true);
    assert_eq!(summary["status"], "simulated");
    assert_eq!(summary["steps"].as_array().unwrap().len(), 2);

    let log = seen.lock().unwrap();
    assert!(
        log.submits.is_empty(),
        "dry-run must never submit, saw: {:?}",
        log.methods
    );
}
