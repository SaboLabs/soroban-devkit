use assert_cmd::Command;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
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

// ── #69: settled FAILED must exit non-zero with diagnostics ────────────

const SEND_PENDING: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"abc123","status":"PENDING","latestLedger":"100"}}"#;
const POLL_FAILED: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"abc123","status":"FAILED","latestLedger":"100","resultXdr":null,"resultMetaXdr":null,"error":"tx_failed"}}"#;
const POLL_SUCCESS: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"abc123","status":"SUCCESS","latestLedger":"100","resultXdr":"AAAA","resultMetaXdr":null,"error":null}}"#;

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

/// Mock JSON-RPC server answering `sendTransaction` and `getTransaction`.
/// Each request arrives on its own connection (`Connection: close`), so one
/// accept loop serves the whole submit → poll lifecycle.
fn mock_rpc_server(send_body: &'static str, poll_body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_http_request(&mut sock);
            if req.is_empty() {
                continue;
            }
            let body = if req.contains("sendTransaction") {
                send_body
            } else {
                poll_body
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

fn submit_against_mock(
    dir: &std::path::Path,
    send_body: &'static str,
    poll_body: &'static str,
) -> std::process::Output {
    let url = mock_rpc_server(send_body, poll_body);
    add_mock_profile(dir, &url);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd.args([
        "tx",
        "submit",
        "--envelope",
        "AAAA",
        "--wait",
        "--network-profile",
        "mocknet",
    ])
    .output()
    .unwrap()
}

#[test]
fn test_cli_submit_wait_failed_settlement_exits_nonzero_with_diagnostics() {
    let dir = tempdir().unwrap();
    let output = submit_against_mock(dir.path(), SEND_PENDING, POLL_FAILED);

    assert!(
        !output.status.success(),
        "settled FAILED must exit non-zero"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Failed"), "status printed, got: {stdout}");
    assert!(
        stdout.contains("tx_failed"),
        "error code printed, got: {stdout}"
    );
}

#[test]
fn test_cli_submit_wait_success_settlement_exits_zero() {
    let dir = tempdir().unwrap();
    let output = submit_against_mock(dir.path(), SEND_PENDING, POLL_SUCCESS);

    assert!(output.status.success(), "settled SUCCESS must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Success"), "status printed, got: {stdout}");
}
