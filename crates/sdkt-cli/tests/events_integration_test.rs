use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use assert_cmd::Command;
use predicates::prelude::*;

const CONTRACT_ID: &str = "CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK";

fn read_request(sock: &mut TcpStream) -> String {
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

#[derive(Default)]
struct MockRpcSeen {
    requests: Vec<serde_json::Value>,
}

fn mock_events_rpc(latest_ledger: u32) -> (String, Arc<Mutex<MockRpcSeen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(MockRpcSeen::default()));
    let seen_thread = seen.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_request(&mut sock);
            let body = req.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
            let json_body: serde_json::Value =
                serde_json::from_str(body).unwrap_or_else(|_| serde_json::json!({}));
            let method = json_body["method"].as_str().unwrap_or("").to_string();

            {
                let mut s = seen_thread.lock().unwrap();
                s.requests.push(json_body);
            }

            let resp_body = match method.as_str() {
                "getLatestLedger" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"id":"mock","protocolVersion":22,"sequence":{latest_ledger}}}}}"#
                ),
                "getEvents" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"events":[{"ledger":2000,"contractId":"CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK","topic":["AAAAAwAAACo=","AAAAAwAAAAc="],"value":"AAAAAwAAACo="}]}}"#.to_string()
                }
                _ => {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string()
                }
            };

            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            );
            let _ = sock.write_all(header.as_bytes());
        }
    });

    (url, seen)
}

#[test]
fn test_events_format_json() {
    let (rpc_url, _seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--format")
        .arg("json");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("AAAAAwAAACo="))
        .stdout(predicate::str::contains("AAAAAwAAAAc="));
}

#[test]
fn test_events_abi_json_preserves_raw_topics_and_value() {
    let (rpc_url, _seen) = mock_events_rpc(2500);
    let wasm_path = format!("{}/tests/fixtures/us_new.wasm", env!("CARGO_MANIFEST_DIR"));

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--abi")
        .arg(&wasm_path)
        .arg("--format")
        .arg("json");

    let output = cmd.assert().success().get_output().stdout.clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();

    let event = &json[0];

    assert_eq!(
        event["topics"],
        serde_json::json!(["AAAAAwAAACo=", "AAAAAwAAAAc="])
    );
    assert_eq!(event["value"], "AAAAAwAAACo=");

    let decoded = event["decoded"]
        .as_array()
        .expect("decoded should be an array");

    assert!(
        !decoded.is_empty(),
        "decoded event data should not be empty"
    );
}

#[test]
fn test_events_invalid_format() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--format")
        .arg("xml");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid format"));
}

#[test]
fn test_events_help_shows_range_flags() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events").arg("--help");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--start-ledger"))
        .stdout(predicate::str::contains("--end-ledger"));
}

#[test]
fn test_events_inverted_range() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--start-ledger")
        .arg("5000")
        .arg("--end-ledger")
        .arg("1000");

    cmd.assert().failure().stderr(predicate::str::contains(
        "start ledger (5000) cannot be greater than end ledger (1000)",
    ));
}

#[test]
fn test_events_explicit_range() {
    let (rpc_url, seen) = mock_events_rpc(5000);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--start-ledger")
        .arg("1000")
        .arg("--end-ledger")
        .arg("2000");

    cmd.assert().success();

    let captured = seen.lock().unwrap();
    let events_req = captured
        .requests
        .iter()
        .find(|r| r["method"] == "getEvents")
        .expect("expected getEvents request");
    assert_eq!(events_req["params"]["startLedger"], 1000);
    // User-supplied end bound (2000) is converted to inclusive bound by sending end + 1 (2001)
    assert_eq!(events_req["params"]["endLedger"], 2001);
}

#[test]
fn test_events_start_ledger_only() {
    let (rpc_url, seen) = mock_events_rpc(5000);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--start-ledger")
        .arg("1000");

    cmd.assert().success();

    let captured = seen.lock().unwrap();
    let events_req = captured
        .requests
        .iter()
        .find(|r| r["method"] == "getEvents")
        .expect("expected getEvents request");
    assert_eq!(events_req["params"]["startLedger"], 1000);
    // When end-ledger is omitted, endLedger is omitted from the request (RPC queries up to latest)
    assert!(events_req["params"].get("endLedger").is_none());
}

#[test]
fn test_events_end_ledger_only() {
    let (rpc_url, seen) = mock_events_rpc(5000);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--end-ledger")
        .arg("2000");

    cmd.assert().success();

    let captured = seen.lock().unwrap();
    let events_req = captured
        .requests
        .iter()
        .find(|r| r["method"] == "getEvents")
        .expect("expected getEvents request");
    // When start-ledger is omitted, startLedger defaults to end_ledger.saturating_sub(1000).max(1) (2000 - 1000 = 1000)
    assert_eq!(events_req["params"]["startLedger"], 1000);
    // User-supplied end bound (2000) is converted to inclusive bound by sending end + 1 (2001)
    assert_eq!(events_req["params"]["endLedger"], 2001);
}

#[test]
fn test_events_default_lookback() {
    let (rpc_url, seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url);

    cmd.assert().success();

    let captured = seen.lock().unwrap();
    let events_req = captured
        .requests
        .iter()
        .find(|r| r["method"] == "getEvents")
        .expect("expected getEvents request");
    // When both are omitted, startLedger defaults to latest_ledger (2500) - 1000 = 1500
    assert_eq!(events_req["params"]["startLedger"], 1500);
    assert!(events_req["params"].get("endLedger").is_none());
}
