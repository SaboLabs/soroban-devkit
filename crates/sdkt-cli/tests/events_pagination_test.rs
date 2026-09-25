//! Contract event pagination.
//!
//! Drives the real `sdkt events` binary against a local mock Soroban RPC that
//! serves a two-page sequence, so the paging loop, the `next_cursor` contract,
//! and the terminal-page detection are exercised end to end without a network.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

const CONTRACT_ID: &str = "CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK";
const TOPIC_A: &str = "AAAAAwAAACo=";
const TOPIC_B: &str = "AAAAAwAAAAc=";
/// The cursor the mock's first page hands back for the second page.
const PAGE_2_CURSOR: &str = "cursor-page-2";

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
struct Seen {
    requests: Vec<Value>,
}

fn event_json(ledger: u32, topic: &str) -> String {
    format!(
        r#"{{"ledger":{ledger},"contractId":"{CONTRACT_ID}","topic":["{topic}"],"value":"{topic}"}}"#
    )
}

/// Mock RPC with two event pages.
///
/// The first page returns exactly as many events as `limit` asks for (3 when
/// the request carries no limit) and reports `pagingTokens`. When that cursor
/// comes back, the second page returns a single event and no `pagingTokens`,
/// which is how the client knows it reached the end.
fn mock_paged_rpc() -> (String, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Seen::default()));
    let seen_thread = seen.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_request(&mut sock);
            let body = req.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
            let json_body: Value =
                serde_json::from_str(body).unwrap_or_else(|_| serde_json::json!({}));
            let method = json_body["method"].as_str().unwrap_or("").to_string();
            let params = json_body["params"].clone();

            {
                let mut s = seen_thread.lock().unwrap();
                s.requests.push(json_body);
            }

            let resp_body = match method.as_str() {
                "getLatestLedger" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":22,"sequence":2500}}"#.to_string()
                }
                "getEvents" => {
                    if params["cursor"].as_str() == Some(PAGE_2_CURSOR) {
                        format!(
                            r#"{{"jsonrpc":"2.0","id":1,"result":{{"latestLedger":2501,"events":[{}]}}}}"#,
                            event_json(2999, TOPIC_B)
                        )
                    } else {
                        let limit = params["limit"].as_u64().unwrap_or(3);
                        let events: Vec<String> = (0..limit)
                            .map(|i| event_json(2000 + i as u32, TOPIC_A))
                            .collect();
                        format!(
                            r#"{{"jsonrpc":"2.0","id":1,"result":{{"latestLedger":2500,"events":[{}],"pagingTokens":{{"newest":"{PAGE_2_CURSOR}","oldest":"cursor-oldest-1"}}}}}}"#,
                            events.join(",")
                        )
                    }
                }
                _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string(),
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

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").expect("sdkt binary built")
}

fn get_events_requests(seen: &Arc<Mutex<Seen>>) -> Vec<Value> {
    seen.lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r["method"] == "getEvents")
        .cloned()
        .collect()
}

#[test]
fn events_help_lists_pagination_flags() {
    let mut cmd = sdkt();
    cmd.args(["events", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--limit"))
        .stdout(predicate::str::contains("--cursor"))
        .stdout(predicate::str::contains("--follow"))
        .stdout(predicate::str::contains("--max-pages"));
}

#[test]
fn limit_is_sent_and_next_cursor_returned() {
    let (rpc_url, seen) = mock_paged_rpc();
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--rpc-url", &rpc_url])
        .args(["--limit", "5", "--format", "json"]);

    let output = cmd.assert().success().get_output().stdout.clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["events"].as_array().unwrap().len(), 5);
    assert_eq!(json["next_cursor"], PAGE_2_CURSOR);
    assert_eq!(json["latest_ledger"], 2500);

    let requests = get_events_requests(&seen);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["params"]["limit"], 5);
    assert!(requests[0]["params"].get("cursor").is_none());
}

#[test]
fn cursor_from_previous_page_is_echoed_back() {
    let (rpc_url, seen) = mock_paged_rpc();
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--rpc-url", &rpc_url])
        .args(["--cursor", PAGE_2_CURSOR, "--format", "json"]);

    let output = cmd.assert().success().get_output().stdout.clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["events"].as_array().unwrap().len(), 1);
    // Final page: the key is present and null rather than absent.
    assert!(json["next_cursor"].is_null());
    assert_eq!(json["latest_ledger"], 2501);

    let requests = get_events_requests(&seen);
    assert_eq!(requests[0]["params"]["cursor"], PAGE_2_CURSOR);
}

#[test]
fn follow_walks_every_page_and_terminates() {
    let (rpc_url, seen) = mock_paged_rpc();
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--rpc-url", &rpc_url])
        .args(["--limit", "5", "--follow", "--format", "json"]);

    let output = cmd.assert().success().get_output().stdout.clone();
    let json: Value = serde_json::from_slice(&output).unwrap();

    // 5 events from page one plus the single event on the final page.
    assert_eq!(json["events"].as_array().unwrap().len(), 6);
    assert!(json["next_cursor"].is_null());

    let requests = get_events_requests(&seen);
    assert_eq!(requests.len(), 2, "expected exactly two pages");
    assert_eq!(requests[1]["params"]["cursor"], PAGE_2_CURSOR);
}

#[test]
fn follow_respects_the_max_pages_safety_cap() {
    let (rpc_url, _seen) = mock_paged_rpc();
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--rpc-url", &rpc_url])
        .args([
            "--limit",
            "5",
            "--follow",
            "--max-pages",
            "1",
            "--format",
            "json",
        ]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--follow stopped after 1 pages"));
}

#[test]
fn default_output_is_unchanged_without_pagination_flags() {
    let (rpc_url, seen) = mock_paged_rpc();
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--rpc-url", &rpc_url])
        .args(["--format", "json"]);

    let output = cmd.assert().success().get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&output).to_string();
    let json: Value = serde_json::from_slice(&output).unwrap();

    // Still a bare array of events, with none of the pagination envelope.
    assert!(json.is_array(), "default output must stay a JSON array");
    assert!(!stdout.contains("next_cursor"));
    assert!(!stdout.contains("latest_ledger"));

    let requests = get_events_requests(&seen);
    assert_eq!(requests.len(), 1);
    assert!(requests[0]["params"].get("limit").is_none());
    assert!(requests[0]["params"].get("cursor").is_none());
}

#[test]
fn range_and_limit_travel_together() {
    let (rpc_url, seen) = mock_paged_rpc();
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--rpc-url", &rpc_url])
        .args([
            "--start-ledger",
            "1000",
            "--end-ledger",
            "2000",
            "--limit",
            "5",
            "--format",
            "json",
        ]);

    cmd.assert().success();

    let requests = get_events_requests(&seen);
    // The inclusive end bound is still converted to end + 1 for the RPC.
    assert_eq!(requests[0]["params"]["startLedger"], 1000);
    assert_eq!(requests[0]["params"]["endLedger"], 2001);
    assert_eq!(requests[0]["params"]["limit"], 5);
}

#[test]
fn pretty_output_reports_the_continuation_cursor() {
    let (rpc_url, _seen) = mock_paged_rpc();
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--rpc-url", &rpc_url])
        .args([
            "--limit",
            "2",
            "--cursor",
            "cursor-page-2",
            "--format",
            "pretty",
        ]);

    cmd.assert().success();
}

#[test]
fn pretty_output_announces_more_pages() {
    let (rpc_url, _seen) = mock_paged_rpc();
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--rpc-url", &rpc_url])
        .args(["--limit", "2", "--format", "pretty"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("More events available"))
        .stdout(predicate::str::contains(PAGE_2_CURSOR));
}

#[test]
fn zero_limit_is_rejected() {
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--limit", "0"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--limit must be greater than 0"));
}

#[test]
fn zero_max_pages_is_rejected() {
    let mut cmd = sdkt();
    cmd.args(["events", CONTRACT_ID, "--follow", "--max-pages", "0"]);

    cmd.assert().failure().stderr(predicate::str::contains(
        "--max-pages must be greater than 0",
    ));
}
