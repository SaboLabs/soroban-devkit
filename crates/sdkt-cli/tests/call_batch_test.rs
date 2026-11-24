//! — `call --batch` tests (hermetic).
//!
//! `sdkt call <contract> --batch FILE` reads a JSON array of read-only
//! calls, executes each through the single-call simulation path, and prints
//! one JSON object keyed by function name (or each entry's `key`). All
//! network interaction is served by an in-process mock JSON-RPC server. No
//! live Testnet is required.

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

/// A real contractspecv0 WASM fixture declaring `hello() -> u32` and
/// `increment() -> u32`.
static WASM_WITH_RETURN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_new.wasm");

/// A deployed-testnet contract StrKey (used only as an identifier; the mock
/// answers every simulation regardless of the contract id).
const CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

/// ScVal::U32(42) in base64 XDR.
const MOCK_SCVAL_U32_42: &str = "AAAAAwAAACo=";
/// ScVal::U32(7) in base64 XDR.
const MOCK_SCVAL_U32_7: &str = "AAAAAwAAAAc=";

fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
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

fn sim_ok_response(scval_b64: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"results\":[{{\"xdr\":\"{scval_b64}\",\"auth\":[]}}],\"latestLedger\":\"12345\",\"events\":[],\"cost\":{{\"cpuInsns\":\"1000\",\"memBytes\":\"2000\"}}}}}}"
    )
}

const SIM_ERR_RESPONSE: &str = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"error\":\"ContractError(1)\",\"latestLedger\":\"12345\",\"events\":[]}}";

/// Mock JSON-RPC server for batch calls.
///
/// Every `simulateTransaction` request is answered in call order: the first
/// returns u32(42), the second u32(7), later ones u32(42) again. When
/// `fail_on` is `Some(n)`, the nth simulation instead returns a simulation
/// error (`ContractError(1)`).
fn mock_batch_rpc(fail_on: Option<usize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let count = Arc::new(AtomicUsize::new(0));

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            let body = if req.contains("\"simulateTransaction\"") {
                let call_no = count.fetch_add(1, Ordering::SeqCst) + 1;
                if fail_on == Some(call_no) {
                    SIM_ERR_RESPONSE.to_string()
                } else if call_no == 2 {
                    sim_ok_response(MOCK_SCVAL_U32_7)
                } else {
                    sim_ok_response(MOCK_SCVAL_U32_42)
                }
            } else {
                r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                    .to_string()
            };

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    thread::sleep(Duration::from_millis(50));
    url
}

fn write_batch(dir: &std::path::Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn batch_help_documents_batch_flag() {
    sdkt_isolated(tempdir().unwrap().path())
        .args(["call", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--batch"));
}

#[test]
fn batch_two_entries_return_keyed_results() {
    let dir = tempdir().unwrap();
    let url = mock_batch_rpc(None);
    add_mock_profile(dir.path(), &url);
    let batch = write_batch(
        dir.path(),
        "two.json",
        r#"[{"function": "hello", "key": "first"}, {"function": "increment", "key": "second"}]"#,
    );

    let output = sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "--batch",
            batch.to_str().unwrap(),
            "--network-profile",
            "mocknet",
            "--abi",
            WASM_WITH_RETURN,
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

    assert_eq!(parsed["first"], "u32(42)");
    assert_eq!(parsed["second"], "u32(7)");
    // File order is preserved in the printed object.
    assert!(
        stdout.find("\"first\"").unwrap() < stdout.find("\"second\"").unwrap(),
        "keys must appear in file order, got: {stdout}"
    );
}

#[test]
fn batch_failing_second_entry_stops_with_nonzero_exit() {
    let dir = tempdir().unwrap();
    let url = mock_batch_rpc(Some(2));
    add_mock_profile(dir.path(), &url);
    let batch = write_batch(
        dir.path(),
        "fail-second.json",
        r#"[{"function": "hello", "key": "first"}, {"function": "increment", "key": "second"}]"#,
    );

    let output = sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "--batch",
            batch.to_str().unwrap(),
            "--network-profile",
            "mocknet",
            "--abi",
            WASM_WITH_RETURN,
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "batch with a failing second entry must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("batch entry 1"),
        "error must name the failing entry index, got: {stderr}"
    );
    assert!(
        stderr.contains("increment"),
        "error must name the failing function, got: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains('{'),
        "a failed batch must print no result object, got: {stdout}"
    );
}

#[test]
fn batch_with_positional_function_is_usage_error() {
    let dir = tempdir().unwrap();
    let batch = write_batch(dir.path(), "one.json", r#"[{"function": "hello"}]"#);

    sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "hello",
            "--batch",
            batch.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn batch_with_args_is_usage_error() {
    let dir = tempdir().unwrap();
    let batch = write_batch(dir.path(), "one.json", r#"[{"function": "hello"}]"#);

    sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "--batch",
            batch.to_str().unwrap(),
            "--args",
            "u32:1",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn batch_duplicate_keys_rejected_before_network() {
    let dir = tempdir().unwrap();
    // Unreachable RPC: if the batch tried the network first, the failure
    // would be a connection error instead of the duplicate-key error.
    add_mock_profile(dir.path(), "http://127.0.0.1:1");
    let batch = write_batch(
        dir.path(),
        "dup.json",
        r#"[{"function": "hello"}, {"function": "increment", "key": "hello"}]"#,
    );

    sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "--batch",
            batch.to_str().unwrap(),
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("duplicate key"))
        .stderr(predicate::str::contains("batch entry 1"));
}

#[test]
fn batch_unknown_function_rejected_with_abi() {
    let dir = tempdir().unwrap();
    let url = mock_batch_rpc(None);
    add_mock_profile(dir.path(), &url);
    let batch = write_batch(
        dir.path(),
        "unknown.json",
        r#"[{"function": "definitely_not_a_function"}]"#,
    );

    sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "--batch",
            batch.to_str().unwrap(),
            "--network-profile",
            "mocknet",
            "--abi",
            WASM_WITH_RETURN,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("batch entry 0"))
        .stderr(predicate::str::contains("unknown function"));
}

#[test]
fn batch_pretty_output_prints_labeled_sections() {
    let dir = tempdir().unwrap();
    let url = mock_batch_rpc(None);
    add_mock_profile(dir.path(), &url);
    let batch = write_batch(
        dir.path(),
        "two.json",
        r#"[{"function": "hello", "key": "first"}, {"function": "increment", "key": "second"}]"#,
    );

    sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "--batch",
            batch.to_str().unwrap(),
            "--network-profile",
            "mocknet",
            "--abi",
            WASM_WITH_RETURN,
            "--format",
            "pretty",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("== first =="))
        .stdout(predicate::str::contains("== second =="))
        .stdout(predicate::str::contains("Result:    u32(42)"))
        .stdout(predicate::str::contains("Result:    u32(7)"));
}
