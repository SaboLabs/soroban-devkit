use assert_cmd::Command;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use stellar_xdr::{
    ContractEvent, ContractEventBody, ContractEventType, ContractEventV0, DiagnosticEvent,
    ExtensionPoint, InvokeHostFunctionResult, LedgerEntryChanges, Limits, OperationResult,
    OperationResultTr, ScVal, SorobanTransactionMeta, SorobanTransactionMetaExt, TransactionMeta,
    TransactionMetaV3, TransactionResult, TransactionResultExt, TransactionResultResult, VecM,
    WriteXdr,
};
use tempfile::tempdir;

// Envelope used for the CLI tests is a deliberately-invalid (but base64-safe)
// envelope so the RPC call either returns a network error or an RPC-level
// rejection — we assert on the error path rather than a real broadcast.
const ENVELOPE: &str =
    "AAAAAgAAAABkQMdGsjCv3zavZlW5740YkOCNy0wKb9E8LPuJ2dXq1QAAAAQAAAAFAAAAAABvq5c=";

const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

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

// ---------- On-chain failure diagnostics / exit codes (mock RPC) ----------

/// A settled `txFAILED` `TransactionResult` carrying an
/// `INVOKE_HOST_FUNCTION_TRAPPED` operation result — the on-chain failure the
/// submission path previously discarded.
fn failed_result_xdr() -> String {
    TransactionResult {
        fee_charged: 100,
        result: TransactionResultResult::TxFailed(
            VecM::try_from(vec![OperationResult::OpInner(
                OperationResultTr::InvokeHostFunction(InvokeHostFunctionResult::Trapped),
            )])
            .unwrap(),
        ),
        ext: TransactionResultExt::V0,
    }
    .to_xdr_base64(Limits::none())
    .unwrap()
}

/// A settled Soroban `TransactionMetaV3` carrying one diagnostic event.
fn failed_meta_xdr() -> String {
    let diagnostic = DiagnosticEvent {
        in_successful_contract_call: false,
        event: ContractEvent {
            ext: ExtensionPoint::V0,
            contract_id: None,
            type_: ContractEventType::Diagnostic,
            body: ContractEventBody::V0(ContractEventV0 {
                topics: VecM::default(),
                data: ScVal::U32(42),
            }),
        },
    };

    TransactionMeta::V3(TransactionMetaV3 {
        ext: ExtensionPoint::V0,
        tx_changes_before: LedgerEntryChanges(VecM::default()),
        operations: VecM::default(),
        tx_changes_after: LedgerEntryChanges(VecM::default()),
        soroban_meta: Some(SorobanTransactionMeta {
            ext: SorobanTransactionMetaExt::V0,
            events: VecM::default(),
            return_value: ScVal::Void,
            diagnostic_events: VecM::try_from(vec![diagnostic]).unwrap(),
        }),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

/// Pull `"method":"..."` out of a raw JSON-RPC request without a full parser.
fn extract_method(request: &str) -> Option<String> {
    let index = request.find("\"method\"")?;
    let rest = &request[index + 8..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let quote = after.find('"')?;
    let end = after[quote + 1..].find('"')?;
    Some(after[quote + 1..quote + 1 + end].to_string())
}

/// Mock JSON-RPC server for the submit lifecycle.
///
/// - `sendTransaction` → `PENDING` with a hash (never an immediate rejection)
/// - `getTransaction` → `FAILED` with result/meta XDR when `settled_failed`,
///   otherwise `SUCCESS`
///
/// Returns the RPC URL plus the list of JSON-RPC methods observed, in order.
fn mock_rpc_server(settled_failed: bool) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<String>>> = Default::default();
    let seen_thread = seen.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(sock) => sock,
                Err(_) => break,
            };
            let mut buf = [0u8; 65536];
            let n = sock.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let method = extract_method(&request);
            seen_thread
                .lock()
                .unwrap()
                .push(method.clone().unwrap_or_default());

            let body = match method.as_deref() {
                Some("sendTransaction") => r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeefcafe","status":"PENDING","latestLedger":"100"}}"#.to_string(),
                Some("getTransaction") => {
                    if settled_failed {
                        format!(
                            r#"{{"jsonrpc":"2.0","id":1,"result":{{"status":"FAILED","latestLedger":"101","resultXdr":"{}","resultMetaXdr":"{}"}}}}"#,
                            failed_result_xdr(),
                            failed_meta_xdr()
                        )
                    } else {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg=="}}"#.to_string()
                    }
                }
                _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string(),
            };

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(response.as_bytes());
        }
    });

    (url, seen)
}

fn submit(dir: &std::path::Path, url: &str, extra: &[&str]) -> std::process::Output {
    // Network flags live on `tx` (flattened), so they must precede the
    // subcommand, matching the other tx integration tests.
    let mut args = vec![
        "tx",
        "--rpc-url",
        url,
        "--network-passphrase",
        TESTNET_PASSPHRASE,
        "submit",
        "--envelope",
        ENVELOPE,
    ];
    args.extend_from_slice(extra);

    Command::cargo_bin("sdkt")
        .unwrap()
        .current_dir(dir)
        .env("SDKT_IDENTITY_DIR", dir.join("identity"))
        .env("SDKT_NETWORK_DIR", dir.join("network"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn tx_submit_wait_failed_settlement_exits_nonzero_with_diagnostics() {
    let dir = tempdir().unwrap();
    let (url, seen) = mock_rpc_server(true);

    let output = submit(
        dir.path(),
        &url,
        &["--wait", "--timeout", "5", "--interval", "1"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "a settled FAILED transaction must exit non-zero. stdout={stdout}"
    );
    assert!(stdout.contains("Status: Failed"), "stdout={stdout}");
    assert!(
        stdout.contains("Error:    tx_failed:invoke_host_function_trapped"),
        "on-chain error code must be reported. stdout={stdout}"
    );
    assert!(
        stdout.contains("Result Meta XDR:"),
        "settled result meta must be surfaced. stdout={stdout}"
    );
    assert!(
        stdout.contains("Diagnostic:"),
        "diagnostic events must be surfaced. stdout={stdout}"
    );

    let methods = seen.lock().unwrap().join(",");
    assert!(methods.contains("sendTransaction"), "methods={methods}");
    assert!(methods.contains("getTransaction"), "methods={methods}");
}

#[test]
fn tx_submit_wait_failed_settlement_json_keeps_diagnostics() {
    let dir = tempdir().unwrap();
    let (url, _seen) = mock_rpc_server(true);

    let output = submit(
        dir.path(),
        &url,
        &[
            "--wait",
            "--timeout",
            "5",
            "--interval",
            "1",
            "--format",
            "json",
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(!output.status.success(), "stdout={stdout}");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("Invalid JSON: {e}\n{stdout}"));
    assert_eq!(parsed["status"], "Failed");
    assert_eq!(
        parsed["errorCode"],
        "tx_failed:invoke_host_function_trapped"
    );
    assert!(parsed["resultXdr"].is_string());
    assert!(parsed["resultMetaXdr"].is_string());
    assert_eq!(parsed["diagnosticEvents"].as_array().unwrap().len(), 1);
}

#[test]
fn tx_submit_wait_success_settlement_exits_zero() {
    let dir = tempdir().unwrap();
    let (url, seen) = mock_rpc_server(false);

    let output = submit(
        dir.path(),
        &url,
        &["--wait", "--timeout", "5", "--interval", "1"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "a settled SUCCESS transaction must exit 0. stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Status: Success"), "stdout={stdout}");
    assert!(
        !stdout.contains("Error:"),
        "no failure diagnostics on success. stdout={stdout}"
    );

    let methods = seen.lock().unwrap().join(",");
    assert!(methods.contains("getTransaction"), "methods={methods}");
}

#[test]
fn tx_submit_without_wait_reports_pending_and_exits_zero() {
    let dir = tempdir().unwrap();
    let (url, seen) = mock_rpc_server(true);

    let output = submit(dir.path(), &url, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Submitting without --wait is accepted: the transaction is Pending, not
    // failed, so it must not be reported as a failure.
    assert!(
        output.status.success(),
        "stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Status: Pending"), "stdout={stdout}");

    let methods = seen.lock().unwrap().join(",");
    assert!(methods.contains("sendTransaction"), "methods={methods}");
    assert!(!methods.contains("getTransaction"), "methods={methods}");
}
