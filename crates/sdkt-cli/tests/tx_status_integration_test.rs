//! `tx status` CLI tests against a stubbed JSON-RPC server (#554).
//!
//! The mock answers `getTransaction` from a scripted queue of responses so
//! each test controls exactly what the network reports — no live node, no
//! identity, no funds.

use assert_cmd::Command;
use base64::Engine as _;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use stellar_xdr::{
    ContractEvent, DiagnosticEvent, InvokeHostFunctionResult, Limits, OperationResult,
    OperationResultTr, ScVal, SorobanTransactionMeta, TransactionMeta, TransactionMetaV3,
    TransactionResult, TransactionResultExt, TransactionResultResult, WriteXdr,
};
use tempfile::tempdir;

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn success_result_xdr() -> String {
    let tx = TransactionResult {
        fee_charged: 100,
        result: TransactionResultResult::TxSuccess(
            vec![OperationResult::OpInner(
                OperationResultTr::InvokeHostFunction(InvokeHostFunctionResult::Success(
                    stellar_xdr::Hash::default(),
                )),
            )]
            .try_into()
            .unwrap(),
        ),
        ext: TransactionResultExt::V0,
    };
    base64::engine::general_purpose::STANDARD.encode(tx.to_xdr(Limits::none()).unwrap())
}

fn trapped_result_xdr() -> String {
    let tx = TransactionResult {
        fee_charged: 100,
        result: TransactionResultResult::TxFailed(
            vec![OperationResult::OpInner(
                OperationResultTr::InvokeHostFunction(InvokeHostFunctionResult::Trapped),
            )]
            .try_into()
            .unwrap(),
        ),
        ext: TransactionResultExt::V0,
    };
    base64::engine::general_purpose::STANDARD.encode(tx.to_xdr(Limits::none()).unwrap())
}

/// A V3 meta holding one ordinary contract event and one diagnostic event,
/// plus each event's own base64 XDR.
/// The Soroban meta carries the invoke return value `u32(42)`, as a real
/// settled invoke would (protocol 30 keeps the return value in the meta).
/// The diagnostic event is a `DiagnosticEvent` in
/// `soroban_meta.diagnostic_events` — the contract event next to it is NOT
/// a diagnostic and must never be reported as one.
fn meta_with_one_event() -> (String, String, String) {
    let event = ContractEvent::default();
    let diagnostic = DiagnosticEvent {
        in_successful_contract_call: false,
        event: ContractEvent::default(),
    };
    let meta = TransactionMeta::V3(TransactionMetaV3 {
        soroban_meta: Some(SorobanTransactionMeta {
            events: vec![event.clone()].try_into().unwrap(),
            diagnostic_events: vec![diagnostic.clone()].try_into().unwrap(),
            return_value: ScVal::U32(42),
            ..Default::default()
        }),
        ..Default::default()
    });
    let meta_b64 =
        base64::engine::general_purpose::STANDARD.encode(meta.to_xdr(Limits::none()).unwrap());
    let event_b64 =
        base64::engine::general_purpose::STANDARD.encode(event.to_xdr(Limits::none()).unwrap());
    let diagnostic_b64 = base64::engine::general_purpose::STANDARD
        .encode(diagnostic.to_xdr(Limits::none()).unwrap());
    (meta_b64, event_b64, diagnostic_b64)
}

/// A `DiagnosticEvent` distinct from the one in `meta_with_one_event`
/// (different `in_successful_contract_call` flag), for the RPC
/// `diagnosticEventsXdr` field.
fn rpc_diagnostic_event_b64() -> String {
    let diagnostic = DiagnosticEvent {
        in_successful_contract_call: true,
        event: ContractEvent::default(),
    };
    base64::engine::general_purpose::STANDARD.encode(diagnostic.to_xdr(Limits::none()).unwrap())
}

fn get_tx_body(status: &str, result_xdr: Option<&str>, result_meta_xdr: Option<&str>) -> String {
    get_tx_body_full(status, result_xdr, result_meta_xdr, None, None)
}

/// Full `getTransaction` result builder: `error` overrides the default
/// error string for FAILED (`tx_failed`), `diagnostic_events_xdr` sets the
/// RPC's own `diagnosticEventsXdr` field. Settled responses carry the
/// inclusion `ledger` (100) alongside `latestLedger` (100), like a real RPC;
/// PENDING / NOT_FOUND carry no inclusion ledger.
fn get_tx_body_full(
    status: &str,
    result_xdr: Option<&str>,
    result_meta_xdr: Option<&str>,
    error: Option<Option<&str>>,
    diagnostic_events_xdr: Option<&str>,
) -> String {
    let settled = status == "SUCCESS" || status == "FAILED";
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": {
            "hash": HASH, "status": status,
            "ledger": if settled { Some("100") } else { None },
            "latestLedger": "100",
            "resultXdr": result_xdr, "resultMetaXdr": result_meta_xdr,
            "error": match error {
                Some(e) => e,
                None => if status == "FAILED" { Some("tx_failed") } else { None },
            },
            "diagnosticEventsXdr": diagnostic_events_xdr.map(|d| vec![d]),
        }
    })
    .to_string()
}

/// `getTransaction` result with no `status` field at all: an unreadable RPC
/// response, which must exit 2 rather than read as PENDING.
fn get_tx_body_no_status() -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "hash": HASH, "latestLedger": "100" }
    })
    .to_string()
}

fn read_http_request(sock: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = sock.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end]
                .lines()
                .filter_map(|l| l.split_once(':'))
                .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).to_string()
}

/// Mock JSON-RPC server answering `getTransaction` from a scripted queue.
/// Each request pops the next response; once the queue is exhausted the last
/// response repeats, so a `--wait` follow-up lookup never starves.
///
/// The mock validates the request before serving anything: the method must
/// be `getTransaction` and `params.hash` must be the hash under test.
/// Anything else gets a JSON-RPC error instead of a scripted response, so a
/// test can never pass by the CLI asking for the wrong thing.
fn mock_rpc_server(responses: Vec<String>) -> String {
    let queue: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(responses.into()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_http_request(&mut sock);
            if req.is_empty() {
                continue;
            }
            let body = {
                let valid = req
                    .find("\r\n\r\n")
                    .and_then(|i| serde_json::from_str::<serde_json::Value>(&req[i + 4..]).ok())
                    .map(|v| {
                        v.get("method").and_then(|m| m.as_str()) == Some("getTransaction")
                            && v.get("params")
                                .and_then(|p| p.get("hash"))
                                .and_then(|h| h.as_str())
                                == Some(HASH)
                    })
                    .unwrap_or(false);
                if !valid {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"unexpected method or hash"}}"#
                        .to_string()
                } else {
                    let mut q = queue.lock().unwrap();
                    if q.len() > 1 {
                        q.pop_front().unwrap()
                    } else {
                        q.front().cloned().unwrap_or_default()
                    }
                }
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn add_mock_profile(dir: &std::path::Path, rpc_url: &str) {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd.args([
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

fn status_against_mock(
    dir: &std::path::Path,
    responses: Vec<String>,
    extra_args: &[&str],
) -> std::process::Output {
    let url = mock_rpc_server(responses);
    add_mock_profile(dir, &url);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd.arg("tx").arg("status").arg(HASH);
    cmd.args(extra_args);
    cmd.args(["--network-profile", "mocknet"]);
    cmd.output().unwrap()
}

#[test]
fn test_cli_tx_status_success_prints_success_and_exits_zero() {
    let dir = tempdir().unwrap();
    let (meta_b64, _, _) = meta_with_one_event();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body(
            "SUCCESS",
            Some(&success_result_xdr()),
            Some(&meta_b64),
        )],
        &[],
    );

    assert!(output.status.success(), "SUCCESS must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Status: SUCCESS"), "got: {stdout}");
    assert!(stdout.contains(&format!("Hash: {HASH}")), "got: {stdout}");
    assert!(stdout.contains("Ledger: 100"), "got: {stdout}");
    assert!(
        stdout.contains("Latest Ledger: 100"),
        "inclusion ledger and latest ledger are separate, got: {stdout}"
    );
    assert!(stdout.contains("Result:"), "got: {stdout}");
    assert!(
        stdout.contains("tx_success"),
        "decoded result printed, got: {stdout}"
    );
}

#[test]
fn test_cli_tx_status_failed_exits_one_with_diagnostics() {
    let dir = tempdir().unwrap();
    let (meta_b64, event_b64, diagnostic_b64) = meta_with_one_event();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body(
            "FAILED",
            Some(&trapped_result_xdr()),
            Some(&meta_b64),
        )],
        &[],
    );

    assert_eq!(output.status.code(), Some(1), "FAILED must exit 1");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Status: FAILED"), "got: {stdout}");
    assert!(
        stdout.contains("tx_failed"),
        "error code printed, got: {stdout}"
    );
    assert!(stdout.contains("Diagnostics:"), "got: {stdout}");
    assert!(
        stdout.contains(&diagnostic_b64),
        "diagnostic event printed, got: {stdout}"
    );
    assert!(
        !stdout.contains(&event_b64),
        "ordinary contract events are not diagnostics, got: {stdout}"
    );
}

#[test]
fn test_cli_tx_status_not_found_exits_two() {
    let dir = tempdir().unwrap();
    let output = status_against_mock(dir.path(), vec![get_tx_body("NOT_FOUND", None, None)], &[]);

    assert_eq!(output.status.code(), Some(2), "NOT_FOUND must exit 2");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Status: NOT_FOUND"), "got: {stdout}");
}

#[test]
fn test_cli_tx_status_pending_without_wait_exits_zero() {
    let dir = tempdir().unwrap();
    let output = status_against_mock(dir.path(), vec![get_tx_body("PENDING", None, None)], &[]);

    assert!(
        output.status.success(),
        "PENDING without --wait must exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Status: PENDING"), "got: {stdout}");
}

#[test]
fn test_cli_tx_status_json_output_parses_with_hash_status_result() {
    let dir = tempdir().unwrap();
    let (meta_b64, _, _) = meta_with_one_event();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body(
            "SUCCESS",
            Some(&success_result_xdr()),
            Some(&meta_b64),
        )],
        &["--format", "json"],
    );

    assert!(output.status.success(), "JSON SUCCESS must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"));
    assert_eq!(v["hash"], HASH);
    assert_eq!(v["status"], "SUCCESS");
    assert_eq!(v["ledger"], "100", "inclusion ledger, got: {stdout}");
    assert_eq!(v["latestLedger"], "100", "got: {stdout}");
    assert_eq!(v["result"]["fee_charged"], "100");
    assert!(v["result_meta"].is_object(), "got: {stdout}");
}

#[test]
fn test_cli_tx_status_malformed_hash_exits_two_without_rpc() {
    // No mock server and no network profile at all: the hash check runs
    // before any RPC client is resolved, so this must fail on validation
    // alone.
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let output = cmd
        .arg("tx")
        .arg("status")
        .arg("not-a-hash")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "malformed hash must exit 2");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid transaction hash"),
        "problem named, got: {stderr}"
    );
}

#[test]
fn test_cli_tx_status_wait_polls_until_settlement() {
    let dir = tempdir().unwrap();
    let (meta_b64, _, _) = meta_with_one_event();
    let output = status_against_mock(
        dir.path(),
        vec![
            get_tx_body("PENDING", None, None),
            get_tx_body("SUCCESS", Some(&success_result_xdr()), Some(&meta_b64)),
        ],
        &["--wait", "--timeout", "10", "--interval", "1"],
    );

    assert!(output.status.success(), "--wait must exit 0 once settled");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Status: SUCCESS"), "got: {stdout}");
}

#[test]
fn test_cli_tx_status_wait_timeout_exits_two() {
    let dir = tempdir().unwrap();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body("PENDING", None, None)],
        &["--wait", "--timeout", "2", "--interval", "1"],
    );

    assert_eq!(output.status.code(), Some(2), "wait timeout must exit 2");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("timed out"), "got: {stderr}");
}

#[test]
fn test_cli_tx_status_corrupt_result_xdr_exits_two_naming_the_hash() {
    let dir = tempdir().unwrap();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body("SUCCESS", Some("not-base64!!!"), None)],
        &[],
    );

    assert_eq!(
        output.status.code(),
        Some(2),
        "corrupt result_xdr must exit 2"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(HASH), "hash named, got: {stderr}");
    assert!(stderr.contains("invalid result_xdr"), "got: {stderr}");
}

fn wasm_fixture_path() -> String {
    format!("{}/tests/fixtures/us_new.wasm", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn test_cli_tx_status_abi_decodes_invoke_return_value() {
    let dir = tempdir().unwrap();
    let (meta_b64, _, _) = meta_with_one_event();
    let wasm_path = wasm_fixture_path();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body(
            "SUCCESS",
            Some(&success_result_xdr()),
            Some(&meta_b64),
        )],
        &["--abi", wasm_path.as_str()],
    );

    assert!(output.status.success(), "ABI SUCCESS must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Decoded Result: u32(42)"),
        "ABI-decoded return value printed, got: {stdout}"
    );
}

#[test]
fn test_cli_tx_status_abi_json_has_decoded_result() {
    let dir = tempdir().unwrap();
    let (meta_b64, _, _) = meta_with_one_event();
    let wasm_path = wasm_fixture_path();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body(
            "SUCCESS",
            Some(&success_result_xdr()),
            Some(&meta_b64),
        )],
        &["--abi", wasm_path.as_str(), "--format", "json"],
    );

    assert!(output.status.success(), "ABI JSON SUCCESS must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"));
    assert_eq!(v["decodedResult"]["label"], "u32(42)", "got: {stdout}");
}

#[test]
fn test_cli_tx_status_abi_mutual_exclusion_exits_two() {
    // The --abi / --abi-contract check runs before any RPC call.
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let output = cmd
        .args([
            "tx",
            "status",
            HASH,
            "--abi",
            "a.wasm",
            "--abi-contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
        ])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "conflicting ABI flags are a usage error and must exit 2, not 1"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("only one of --abi or --abi-contract"),
        "got: {stderr}"
    );
}

#[test]
fn test_cli_tx_status_unknown_wire_status_exits_two() {
    let dir = tempdir().unwrap();
    let output = status_against_mock(dir.path(), vec![get_tx_body("WEIRD", None, None)], &[]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "an unknown wire status is unreadable, not PENDING"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unrecognized transaction status"),
        "got: {stderr}"
    );
}

#[test]
fn test_cli_tx_status_missing_wire_status_exits_two() {
    let dir = tempdir().unwrap();
    let output = status_against_mock(dir.path(), vec![get_tx_body_no_status()], &[]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "a response without a status field is unreadable, not PENDING"
    );
}

#[test]
fn test_cli_tx_status_invalid_format_exits_two() {
    let dir = tempdir().unwrap();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body("SUCCESS", Some(&success_result_xdr()), None)],
        &["--format", "yaml"],
    );

    assert_eq!(
        output.status.code(),
        Some(2),
        "an unsupported --format is a usage error and must exit 2"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Invalid format 'yaml'"), "got: {stderr}");
}

#[test]
fn test_cli_tx_status_abi_skipped_when_no_return_value() {
    // A successful transaction with no invoke return value in the metadata
    // has nothing to decode: a missing --abi file must not fail the report.
    let dir = tempdir().unwrap();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body("SUCCESS", Some(&success_result_xdr()), None)],
        &["--abi", "missing.wasm"],
    );

    assert!(
        output.status.success(),
        "SUCCESS without a return value must exit 0 even with a missing --abi file"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Status: SUCCESS"), "got: {stdout}");
}

#[test]
fn test_cli_tx_status_wait_keeps_settling_response_meta() {
    // The metadata must come from the response that settled the poll. The
    // queue ends with NOT_FOUND: if the CLI issued a follow-up lookup after
    // settling (the old behavior), the meta would be lost.
    let dir = tempdir().unwrap();
    let (meta_b64, _, _) = meta_with_one_event();
    let output = status_against_mock(
        dir.path(),
        vec![
            get_tx_body("PENDING", None, None),
            get_tx_body("SUCCESS", Some(&success_result_xdr()), Some(&meta_b64)),
            get_tx_body("NOT_FOUND", None, None),
        ],
        &[
            "--wait",
            "--timeout",
            "10",
            "--interval",
            "1",
            "--format",
            "json",
        ],
    );

    assert!(output.status.success(), "--wait must exit 0 once settled");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"));
    assert_eq!(v["status"], "SUCCESS");
    assert!(
        v["result_meta"].is_object(),
        "result_meta comes from the settling response, got: {stdout}"
    );
}

#[test]
fn test_cli_tx_status_failed_prefers_rpc_diagnostic_events() {
    // The RPC's own diagnosticEventsXdr wins over the metadata, and the
    // ordinary contract event in the same metadata is never a diagnostic.
    let dir = tempdir().unwrap();
    let (meta_b64, event_b64, _) = meta_with_one_event();
    let rpc_diag = rpc_diagnostic_event_b64();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body_full(
            "FAILED",
            Some(&trapped_result_xdr()),
            Some(&meta_b64),
            None,
            Some(&rpc_diag),
        )],
        &[],
    );

    assert_eq!(output.status.code(), Some(1), "FAILED must exit 1");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Diagnostics:"), "got: {stdout}");
    assert!(
        stdout.contains(&rpc_diag),
        "RPC diagnostic event printed, got: {stdout}"
    );
    assert!(
        !stdout.contains(&event_b64),
        "ordinary contract events are not diagnostics, got: {stdout}"
    );
}

#[test]
fn test_cli_tx_status_failed_derives_error_code_without_rpc_error_string() {
    // A FAILED response with a trapped-invoke result XDR but no `error`
    // string derives the same code the --wait path reports.
    let dir = tempdir().unwrap();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body_full(
            "FAILED",
            Some(&trapped_result_xdr()),
            None,
            Some(None),
            None,
        )],
        &["--format", "json"],
    );

    assert_eq!(output.status.code(), Some(1), "FAILED must exit 1");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"));
    assert_eq!(v["error"], "invoke_host_function_trapped", "got: {stdout}");
}

#[test]
fn test_cli_tx_status_pending_omits_inclusion_ledger() {
    let dir = tempdir().unwrap();
    let output = status_against_mock(
        dir.path(),
        vec![get_tx_body("PENDING", None, None)],
        &["--format", "json"],
    );

    assert!(
        output.status.success(),
        "PENDING without --wait must exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"));
    assert_eq!(v["status"], "PENDING");
    assert!(
        v.get("ledger").is_none(),
        "no inclusion ledger yet, got: {stdout}"
    );
    assert_eq!(v["latestLedger"], "100", "got: {stdout}");

    let dir = tempdir().unwrap();
    let output = status_against_mock(dir.path(), vec![get_tx_body("PENDING", None, None)], &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.lines().any(|l| l.starts_with("Ledger:")),
        "no inclusion ledger line for an unsettled transaction, got: {stdout}"
    );
    assert!(stdout.contains("Latest Ledger: 100"), "got: {stdout}");
}
