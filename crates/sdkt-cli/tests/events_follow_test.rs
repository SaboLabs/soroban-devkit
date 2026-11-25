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

fn mock_follow_rpc(
    latest_ledger: u32,
    events_json: &'static str,
) -> (String, Arc<Mutex<MockRpcSeen>>) {
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
                "getEvents" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"events":{events_json},"latestLedger":{latest_ledger}}}}}"#
                ),
                _ => {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                        .to_string()
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
fn follow_max_events_1_prints_one_event_and_exits_0() {
    let events = r#"[
        {"ledger":2000,"contractId":"CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK","topic":["AAAAAwAAACo="],"value":"AAAAAwAAACo="},
        {"ledger":2001,"contractId":"CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK","topic":["AAAAAwAAACo="],"value":"AAAAAwAAACs="}
    ]"#;
    let (url, _seen) = mock_follow_rpc(2000, events);

    Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "events",
            CONTRACT_ID,
            "--follow",
            "--max-events",
            "1",
            "--interval",
            "1",
            "--rpc-url",
            &url,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Following events for"))
        .stdout(predicate::str::contains("Event #1"))
        .stdout(predicate::str::contains("Event #2").not())
        .stdout(predicate::str::contains(
            "Stopped following events: 1 event(s) seen",
        ));
}

#[test]
fn follow_format_json_emits_jsonl() {
    let events = r#"[
        {"ledger":2000,"contractId":"CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK","topic":["AAAAAwAAACo="],"value":"AAAAAwAAACo="},
        {"ledger":2001,"contractId":"CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK","topic":["AAAAAwAAACo="],"value":"AAAAAwAAACs="}
    ]"#;
    let (url, _seen) = mock_follow_rpc(2000, events);

    let output = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "events",
            CONTRACT_ID,
            "--follow",
            "--max-events",
            "2",
            "--interval",
            "1",
            "--format",
            "json",
            "--rpc-url",
            &url,
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Following events for"));
    assert!(stderr.contains("Stopped following events: 2 event(s) seen"));

    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected 2 JSONL lines, got: {stdout}");
    for (idx, line) in lines.iter().enumerate() {
        let obj: serde_json::Value =
            serde_json::from_str(line).expect("each line must be valid JSON");
        assert!(obj.is_object(), "line {idx} must be a JSON object");
        assert_eq!(obj["contract_id"], CONTRACT_ID);
        assert_eq!(obj["ledger"], 2000 + idx as u64);
    }
}

#[test]
fn follow_max_seconds_exits_0_when_elapsed() {
    let (url, _seen) = mock_follow_rpc(2000, "[]");

    Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "events",
            CONTRACT_ID,
            "--follow",
            "--max-seconds",
            "1",
            "--interval",
            "1",
            "--rpc-url",
            &url,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Stopped following events: 0 event(s) seen",
        ));
}

#[test]
fn follow_with_end_ledger_rejected_with_usage_error() {
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["events", CONTRACT_ID, "--follow", "--end-ledger", "2000"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--end-ledger"));
}

#[test]
fn follow_topic_filters_server_side() {
    let events = r#"[
        {"ledger":2000,"contractId":"CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK","topic":["AAAAAwAAACo="],"value":"AAAAAwAAACo="}
    ]"#;
    let (url, seen) = mock_follow_rpc(2000, events);

    Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "events",
            CONTRACT_ID,
            "--follow",
            "--max-events",
            "1",
            "--topic",
            "transfer",
            "--interval",
            "1",
            "--rpc-url",
            &url,
        ])
        .assert()
        .success();

    let s = seen.lock().unwrap();
    let get_events_req = s
        .requests
        .iter()
        .find(|r| r["method"] == "getEvents")
        .expect("expected a getEvents RPC request");
    let topics = &get_events_req["params"]["filters"][0]["topics"];
    assert!(
        topics.is_array() && !topics.as_array().unwrap().is_empty(),
        "expected server-side topic filter in getEvents request, got: {get_events_req}"
    );
    assert_eq!(topics[0][1], "**");
}
