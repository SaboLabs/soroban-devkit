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

// ── `--topic <SYMBOL>` server-side event filtering ──

/// base64 XDR of `ScVal::Symbol("Transfer")`.
const TRANSFER_TOPIC_B64: &str = "AAAADwAAAAhUcmFuc2Zlcg==";

fn events_request(seen: &Arc<Mutex<MockRpcSeen>>) -> serde_json::Value {
    seen.lock()
        .unwrap()
        .requests
        .iter()
        .find(|r| r["method"] == "getEvents")
        .expect("expected getEvents request")
        .clone()
}

/// Write a minimal WASM whose `contractspecv0` section declares one
/// `Transfer` event with prefix topic `transfer` (the `#[contractevent]` default).
fn wasm_with_transfer_event(dir: &std::path::Path) -> std::path::PathBuf {
    use stellar_xdr::{
        Limited, Limits, ScSpecEntry, ScSpecEventDataFormat, ScSpecEventV0, WriteXdr,
    };
    let entry = ScSpecEntry::EventV0(ScSpecEventV0 {
        doc: "".try_into().unwrap(),
        lib: "test".try_into().unwrap(),
        name: "Transfer".try_into().unwrap(),
        prefix_topics: vec!["transfer".try_into().unwrap()].try_into().unwrap(),
        params: vec![].try_into().unwrap(),
        data_format: ScSpecEventDataFormat::SingleValue,
    });
    let name = b"contractspecv0";
    let mut section = vec![name.len() as u8];
    section.extend_from_slice(name);
    let mut buf = Vec::new();
    entry
        .write_xdr(&mut Limited::new(&mut buf, Limits::none()))
        .unwrap();
    section.extend_from_slice(&buf);

    let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x00];
    let mut sz = section.len() as u32;
    while sz >= 0x80 {
        wasm.push((sz as u8 & 0x7f) | 0x80);
        sz >>= 7;
    }
    wasm.push(sz as u8);
    wasm.extend_from_slice(&section);

    let path = dir.join("with_transfer_event.wasm");
    std::fs::write(&path, wasm).unwrap();
    path
}

#[test]
fn test_events_help_shows_topic_flag() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events").arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--topic <SYMBOL>"));
}

#[test]
fn test_events_topic_sends_first_topic_matcher() {
    let (rpc_url, seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--topic")
        .arg("Transfer");
    cmd.assert().success();

    let req = events_request(&seen);
    let filters = req["params"]["filters"].as_array().unwrap();
    assert_eq!(filters.len(), 1);
    assert_eq!(
        filters[0],
        serde_json::json!({
            "type": "contract",
            "contractIds": [CONTRACT_ID],
            "topics": [[TRANSFER_TOPIC_B64, "**"]],
        })
    );
    // Ledger range handling is unaffected by the topic filter.
    assert_eq!(req["params"]["startLedger"], 1500);
}

#[test]
fn test_events_without_topic_sends_no_matcher() {
    let (rpc_url, seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url);
    cmd.assert().success();

    let req = events_request(&seen);
    // Exactly the pre-`--topic` filter shape: no `topics` key at all.
    assert_eq!(
        req["params"]["filters"],
        serde_json::json!([{ "type": "contract", "contractIds": [CONTRACT_ID] }])
    );
}

#[test]
fn test_events_topic_is_not_post_filtered() {
    // The mock ignores `topics` and returns an event whose topic[0] is a u32,
    // not `Transfer`. It must still be printed: filtering is the RPC's job, so
    // the CLI never drops events from the response itself.
    let (rpc_url, _seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--topic")
        .arg("Transfer")
        .arg("--format")
        .arg("json");

    let output = cmd.assert().success().get_output().stdout.clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);
}

#[test]
fn test_events_topic_json_shape_unchanged() {
    let run = |topic: Option<&str>| -> serde_json::Value {
        let (rpc_url, _seen) = mock_events_rpc(2500);
        let mut cmd = Command::cargo_bin("sdkt").unwrap();
        cmd.arg("events")
            .arg(CONTRACT_ID)
            .arg("--rpc-url")
            .arg(&rpc_url)
            .arg("--format")
            .arg("json");
        if let Some(t) = topic {
            cmd.arg("--topic").arg(t);
        }
        let out = cmd.assert().success().get_output().stdout.clone();
        serde_json::from_slice(&out).expect("stdout must be pure JSON")
    };
    assert_eq!(run(Some("Transfer")), run(None));
}

#[test]
fn test_events_topic_invalid_symbol_fails_before_rpc() {
    let (rpc_url, seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--topic")
        .arg("not a symbol");
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--topic"))
        .stderr(predicate::str::contains("invalid symbol"));
    assert!(seen.lock().unwrap().requests.is_empty());
}

#[test]
fn test_events_topic_unknown_to_abi_warns() {
    let dir = tempfile::tempdir().unwrap();
    let wasm = wasm_with_transfer_event(dir.path());
    let (rpc_url, seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--abi")
        .arg(&wasm)
        .arg("--topic")
        .arg("Mint")
        .arg("--format")
        .arg("json");

    let assert = cmd.assert().success().stderr(predicate::str::contains(
        "Warning: event topic 'Mint' is not declared in the contract ABI",
    ));
    // Warning goes to stderr only; stdout stays parseable JSON.
    let stdout = assert.get_output().stdout.clone();
    serde_json::from_slice::<serde_json::Value>(&stdout).unwrap();
    // A warning, not an error: the filtered query is still sent.
    assert!(events_request(&seen)["params"]["filters"][0]["topics"].is_array());
}

#[test]
fn test_events_topic_case_mismatch_suggests_declared_symbol() {
    let dir = tempfile::tempdir().unwrap();
    let wasm = wasm_with_transfer_event(dir.path());
    let (rpc_url, _seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--abi")
        .arg(&wasm)
        .arg("--topic")
        .arg("TRANSFER");
    cmd.assert()
        .success()
        .stderr(predicate::str::contains("did you mean 'transfer'?"));
}

#[test]
fn test_events_topic_known_to_abi_does_not_warn() {
    // `transfer` is the prefix topic, i.e. what `topic[0]` carries on the wire.
    let dir = tempfile::tempdir().unwrap();
    let wasm = wasm_with_transfer_event(dir.path());
    let (rpc_url, _seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--abi")
        .arg(&wasm)
        .arg("--topic")
        .arg("transfer");
    cmd.assert()
        .success()
        .stderr(predicate::str::contains("Warning").not());
}

#[test]
fn test_events_topic_abi_name_warns_with_prefix_topic_hint() {
    // `Transfer` is the ABI event name, but the wire topic is the prefix
    // `transfer`: querying by the name matches nothing, so the user must be told.
    let dir = tempfile::tempdir().unwrap();
    let wasm = wasm_with_transfer_event(dir.path());
    let (rpc_url, seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--abi")
        .arg(&wasm)
        .arg("--topic")
        .arg("Transfer")
        .arg("--format")
        .arg("json");

    let assert = cmd.assert().success().stderr(predicate::str::contains(
        "did you mean 'transfer'? (the wire topic is the prefix topic)",
    ));
    let stdout = assert.get_output().stdout.clone();
    serde_json::from_slice::<serde_json::Value>(&stdout).unwrap();
    // Non-fatal: the query still runs with the symbol the user gave.
    assert_eq!(
        events_request(&seen)["params"]["filters"][0]["topics"],
        serde_json::json!([[TRANSFER_TOPIC_B64, "**"]])
    );
}

#[test]
fn test_events_topic_without_abi_does_not_warn() {
    let (rpc_url, _seen) = mock_events_rpc(2500);
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("events")
        .arg(CONTRACT_ID)
        .arg("--rpc-url")
        .arg(&rpc_url)
        .arg("--topic")
        .arg("Anything");
    cmd.assert()
        .success()
        .stderr(predicate::str::contains("Warning").not());
}
