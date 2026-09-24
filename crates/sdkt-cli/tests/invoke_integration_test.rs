//! Integration tests for `sdkt invoke` — the state-changing end-to-end
//! contract invocation workflow (sequence → simulate → sign → submit → poll).
//!
//! CI-safe: all network interaction is served by an in-process mock JSON-RPC
//! server that routes per-method. No live Testnet, no funded account needed.
//!
//! NOT TESTED here: live Testnet submission (documented in docs/cli.md).

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use tempfile::tempdir;

fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

const VALID_CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

/// LedgerEntry XDR: an account entry for GAAA...WHF (all-zero ed25519 key),
/// balance 10 XLM, seq_num 41 (so next sequence = 42).
const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";

/// SorobanTransactionData XDR: empty footprint, 1000 instructions, 150 stroops
/// resource fee (so total fee = 100 inclusion + 150 = 250).
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

/// Mock JSON-RPC server that routes by method name.
///
/// - `getLedgerEntries` → account entry (sequence fetch)
/// - `simulateTransaction` → transactionData + minResourceFee 150
/// - `sendTransaction` → PENDING with hash
/// - `getTransaction` → SUCCESS (or FAILED when `tx_failed` is set)
///
/// `requests` records each JSON-RPC method received, in order, joined by '\n'.
fn mock_rpc_server(tx_failed: bool) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    mock_rpc_server_with_send_error(tx_failed, false)
}

fn mock_rpc_server_with_send_error(
    tx_failed: bool,
    send_error: bool,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let seen_thread = seen.clone();
    let failed = tx_failed;

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            // Extract the method name from the JSON-RPC request body.
            let method = serde_extract_method(&req);
            seen_thread
                .lock()
                .unwrap()
                .push(method.clone().unwrap_or_default());

            let body = match method.as_deref() {
                Some("getLedgerEntries") => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                ),
                Some("simulateTransaction") => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                ),
                Some("sendTransaction") => {
                    if send_error {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeefcafe","status":"ERROR","latestLedger":"100","errorResult":"tx_bad_auth","errorResultXdr":"AAAA","diagnosticEvents":["AAAAevent"]}}"#.to_string()
                    } else {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeefcafe","status":"PENDING","latestLedger":"100"}}"#.to_string()
                    }
                }
                Some("getTransaction") => {
                    if failed {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"status":"FAILED","latestLedger":"101","resultXdr":"AAAAf////g=="}}"#.to_string()
                    } else {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg=="}}"#.to_string()
                    }
                }
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

/// Pull `"method":"..."` out of a raw JSON-RPC request without a full parser.
fn serde_extract_method(req: &str) -> Option<String> {
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

// ---------- Command registration / help ----------

#[test]
fn invoke_registered_and_shown_in_help() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("--help").assert().success().stdout(
        predicates::str::contains("invoke")
            .and(predicates::str::contains("Invoke a contract function")),
    );
}

#[test]
fn invoke_help_shows_identity_and_args_flags() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("invoke")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("--identity"))
        .stdout(predicates::str::contains("--args"))
        .stdout(predicates::str::contains("--no-wait"));
}

// ---------- Argument parsing ----------

#[test]
fn invoke_missing_function_errors() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args(["invoke", VALID_CONTRACT])
        .assert()
        .failure()
        .stderr(predicates::str::contains("FUNCTION"));
}

#[test]
fn invoke_invalid_arg_type_strict_rejected() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, _seen) = mock_rpc_server(false);
    add_mock_profile(dir.path(), &url);

    sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "set_value",
            "--identity",
            "alice",
            "--network-profile",
            "mocknet",
            "--args",
            "u99:100",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("unknown arg type"));
}

#[test]
fn invoke_arg_without_colon_strict_rejected() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, _seen) = mock_rpc_server(false);
    add_mock_profile(dir.path(), &url);

    sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "set_value",
            "--identity",
            "alice",
            "--network-profile",
            "mocknet",
            "--args",
            "not_typed_value",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("TYPE:VALUE"));
}

#[test]
fn invoke_invalid_u32_value_rejected() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, _seen) = mock_rpc_server(false);
    add_mock_profile(dir.path(), &url);

    sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "set_value",
            "--identity",
            "alice",
            "--network-profile",
            "mocknet",
            "--args",
            "u32:not_a_number",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("invalid u32"));
}

// ---------- Identity / signing path ----------

#[test]
fn invoke_unknown_identity_errors() {
    let dir = tempdir().unwrap();
    let (url, _seen) = mock_rpc_server(false);
    add_mock_profile(dir.path(), &url);

    sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "increment",
            "--identity",
            "ghost",
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("not found"));
}

// ---------- Full lifecycle against the mock ----------

#[test]
fn invoke_success_pretty_output() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, seen) = mock_rpc_server(false);
    add_mock_profile(dir.path(), &url);

    let output = sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "increment",
            "--identity",
            "alice",
            "--network-profile",
            "mocknet",
            "--args",
            "u32:42",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Expected success. stdout={stdout} stderr={stderr}"
    );
    assert!(stdout.contains("Invocation Result:"), "stdout={stdout}");
    assert!(stdout.contains("Status:   SUCCESS"), "stdout={stdout}");
    assert!(stdout.contains("Hash:     deadbeefcafe"), "stdout={stdout}");
    assert!(stdout.contains("Fee:      250 stroops"), "stdout={stdout}");

    // Full lifecycle: sequence → simulate → send → poll.
    let methods = seen.lock().unwrap().join(",");
    assert!(
        methods.contains("getLedgerEntries")
            && methods.contains("simulateTransaction")
            && methods.contains("sendTransaction")
            && methods.contains("getTransaction"),
        "methods={methods}"
    );
}

#[test]
fn invoke_success_json_output() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, _seen) = mock_rpc_server(false);
    add_mock_profile(dir.path(), &url);

    let output = sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "increment",
            "--identity",
            "alice",
            "--network-profile",
            "mocknet",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("Invalid JSON: {e}\n{stdout}"));
    assert_eq!(parsed["status"], "SUCCESS");
    assert_eq!(parsed["hash"], "deadbeefcafe");
    assert_eq!(parsed["fee"], 250);
    assert_eq!(parsed["function"], "increment");
    assert!(parsed.get("errorResultXdr").unwrap().is_null());
}

#[test]
fn invoke_no_wait_returns_pending_without_polling() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, seen) = mock_rpc_server(false);
    add_mock_profile(dir.path(), &url);

    let output = sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "increment",
            "--identity",
            "alice",
            "--network-profile",
            "mocknet",
            "--no-wait",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "no-wait should succeed after submission. stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("Invalid JSON: {e}\n{stdout}"));
    assert_eq!(parsed["status"], "PENDING");
    assert_eq!(parsed["hash"], "deadbeefcafe");

    let methods = seen.lock().unwrap().clone();
    assert!(methods.contains(&"getLedgerEntries".to_string()));
    assert!(methods.contains(&"simulateTransaction".to_string()));
    assert!(methods.contains(&"sendTransaction".to_string()));
    assert!(
        !methods.contains(&"getTransaction".to_string()),
        "methods={methods:?}"
    );
}

#[test]
fn invoke_send_error_json_includes_error_result_xdr() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, seen) = mock_rpc_server_with_send_error(false, true);
    add_mock_profile(dir.path(), &url);

    let output = sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "increment",
            "--identity",
            "alice",
            "--network-profile",
            "mocknet",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "stdout={stdout}");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("Invalid JSON: {e}\n{stdout}"));
    assert_eq!(parsed["status"], "FAILED");
    assert_eq!(parsed["errorCode"], "tx_bad_auth");
    assert_eq!(parsed["errorResultXdr"], "AAAA");
    assert_eq!(parsed["diagnosticEvents"], serde_json::json!(["AAAAevent"]));
    assert!(!seen.lock().unwrap().contains(&"getTransaction".to_string()));
}

#[test]
fn invoke_transaction_failure_nonzero_exit_and_diagnostics() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    let (url, _seen) = mock_rpc_server(true);
    add_mock_profile(dir.path(), &url);

    let output = sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "increment",
            "--identity",
            "alice",
            "--network-profile",
            "mocknet",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "must exit non-zero. stdout={stdout}"
    );
    assert!(stdout.contains("FAILED"), "stdout={stdout}");
}

// ---------- Submission failure path ----------

#[test]
fn invoke_rpc_unreachable_errors_cleanly() {
    let dir = tempdir().unwrap();
    generate_identity(dir.path(), "alice");
    // Port 1 refuses connections — sequence fetch fails before anything signs.
    add_mock_profile(dir.path(), "http://127.0.0.1:1");

    let output = sdkt_isolated(dir.path())
        .args([
            "invoke",
            VALID_CONTRACT,
            "increment",
            "--identity",
            "alice",
            "--network-profile",
            "mocknet",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error"), "stderr={stderr}");
}

// ---------- Existing behavior unchanged ----------

#[test]
fn call_still_works_after_shared_parser_refactor() {
    // `call` (read-only) must keep working through the shared typed-arg parser.
    let dir = tempdir().unwrap();
    let (url, _seen) = mock_rpc_server(false);
    add_mock_profile(dir.path(), &url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "balance",
            "--network-profile",
            "mocknet",
            "--args",
            "u32:7",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Contract:"));
}

#[test]
fn tx_build_still_accepts_passthrough_args() {
    // `tx build` historically passes unknown-type args through as base64 ScVal.
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args([
            "tx",
            "build",
            "--source",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "--sequence",
            "42",
            "--contract",
            VALID_CONTRACT,
            "--function",
            "hello",
            "--arg",
            "AAAAAQ==", // pre-encoded base64 ScVal — no TYPE: prefix
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Transaction Envelope"));
}
