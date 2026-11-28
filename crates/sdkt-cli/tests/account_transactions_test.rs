//! Integration tests for `sdkt account transactions`.
//!
//! A local mock Soroban RPC serves canned `getTransactions` pages (and
//! `getLatestLedger`); no live network access.

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use stellar_xdr::{
    Limits, MuxedAccount, SequenceNumber, Transaction, TransactionEnvelope, TransactionV1Envelope,
    Uint256, WriteXdr,
};
use tempfile::tempdir;

const ADDRESS_A_KEY: [u8; 32] = [0x11; 32];
const ADDRESS_B_KEY: [u8; 32] = [0x22; 32];

fn address_for(pubkey: [u8; 32]) -> String {
    MuxedAccount::Ed25519(Uint256(pubkey)).to_string()
}

fn envelope_xdr_for(pubkey: [u8; 32]) -> String {
    let tx = Transaction {
        source_account: MuxedAccount::Ed25519(Uint256(pubkey)),
        fee: 100,
        seq_num: SequenceNumber(1),
        ..Default::default()
    };
    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: vec![].try_into().unwrap(),
    });
    STANDARD.encode(envelope.to_xdr(Limits::none()).unwrap())
}

fn tx_json(ledger: u32, tx_hash: &str, pubkey: [u8; 32], status: &str) -> serde_json::Value {
    serde_json::json!({
        "envelopeXdr": envelope_xdr_for(pubkey),
        "resultXdr": "AAAA",
        "ledger": ledger,
        "txHash": tx_hash,
        "status": status,
        "feeCharged": "100",
        "applicationOrder": 1,
    })
}

fn page_json(transactions: Vec<serde_json::Value>, cursor: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "transactions": transactions,
        "latestLedger": 500,
        "latestLedgerCloseTimestamp": "2026-01-01T00:00:00Z",
        "oldestLedger": 1,
        "oldestLedgerCloseTimestamp": "2025-12-01T00:00:00Z",
        "cursor": cursor,
    })
}

/// Pages keyed by the request cursor (`None` = first page, ledger mode).
fn two_page_fixture() -> HashMap<Option<String>, serde_json::Value> {
    let mut pages = HashMap::new();
    pages.insert(
        None,
        page_json(
            vec![
                tx_json(100, "hash-b1", ADDRESS_B_KEY, "SUCCESS"),
                tx_json(101, "hash-b2", ADDRESS_B_KEY, "FAILED"),
            ],
            Some("page-2"),
        ),
    );
    pages.insert(
        Some("page-2".to_string()),
        page_json(
            vec![tx_json(102, "hash-a1", ADDRESS_A_KEY, "SUCCESS")],
            None,
        ),
    );
    pages
}

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

/// Serve `getLatestLedger` (sequence 500) and `getTransactions` pages
/// routed by the request cursor. Runs until the listener is dropped.
fn start_mock_server(pages: HashMap<Option<String>, serde_json::Value>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_request(&mut sock);
            if req.is_empty() {
                continue;
            }
            let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
            let request: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let method = request["method"].as_str().unwrap_or("");
            let result = if method == "getLatestLedger" {
                serde_json::json!({
                    "id": "mock",
                    "protocolVersion": 30,
                    "sequence": 500,
                })
            } else if method == "getTransactions" {
                let cursor = request["params"]["pagination"]["cursor"]
                    .as_str()
                    .map(|s| s.to_string());
                pages.get(&cursor).cloned().unwrap_or_else(|| {
                    serde_json::json!({
                        "transactions": [],
                        "latestLedger": 500,
                        "oldestLedger": 1,
                        "cursor": null,
                    })
                })
            } else {
                serde_json::json!({
                    "code": -32601,
                    "message": "method not found",
                })
            };
            let resp_body =
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });
    url
}

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

fn run_transactions(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut full = vec!["account", "transactions"];
    full.extend_from_slice(args);
    full.extend_from_slice(&["--network-profile", "mocknet"]);
    sdkt_isolated(dir).args(&full).output().unwrap()
}

#[test]
fn lists_matches_in_ascending_order_with_accounting() {
    let dir = tempdir().unwrap();
    let url = start_mock_server(two_page_fixture());
    add_mock_profile(dir.path(), &url);
    let address_a = address_for(ADDRESS_A_KEY);

    let output = run_transactions(dir.path(), &[&address_a, "--start-ledger", "100"]);
    assert!(output.status.success(), "Command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The match on the second page is listed; the first page held no match.
    assert!(stdout.contains("hash-a1"));
    assert!(stdout.contains("102"));
    assert!(!stdout.contains("hash-b1"));
    assert!(!stdout.contains("hash-b2"));
    assert!(stdout.contains("Scanned: 3"));
    assert!(stdout.contains("Matched: 1"));
    assert!(stdout.contains("Pages: 2"));
    assert!(stdout.contains("oldestLedger 1"));
    assert!(stdout.contains("latestLedger 500"));
}

#[test]
fn json_output_carries_scan_fields() {
    let dir = tempdir().unwrap();
    let url = start_mock_server(two_page_fixture());
    add_mock_profile(dir.path(), &url);
    let address_a = address_for(ADDRESS_A_KEY);

    let output = run_transactions(
        dir.path(),
        &[&address_a, "--start-ledger", "100", "--format", "json"],
    );
    assert!(output.status.success(), "Command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(parsed["scanned"], 3);
    assert_eq!(parsed["matched"], 1);
    assert_eq!(parsed["pages"], 2);
    assert_eq!(parsed["oldestLedger"], 1);
    assert_eq!(parsed["latestLedger"], 500);
    assert!(parsed["nextCursor"].is_null());
    let txs = parsed["transactions"]
        .as_array()
        .expect("transactions array");
    assert_eq!(txs.len(), 1);
    assert_eq!(txs[0]["txHash"], "hash-a1");
    assert_eq!(txs[0]["ledger"], 102);
    assert_eq!(txs[0]["status"], "SUCCESS");
}

#[test]
fn max_matches_returns_exactly_one_match() {
    let dir = tempdir().unwrap();
    // The first page holds the match, so the scan stops after one page.
    let mut pages = HashMap::new();
    pages.insert(
        None,
        page_json(
            vec![
                tx_json(100, "hash-a1", ADDRESS_A_KEY, "SUCCESS"),
                tx_json(101, "hash-a2", ADDRESS_A_KEY, "SUCCESS"),
                tx_json(102, "hash-b1", ADDRESS_B_KEY, "SUCCESS"),
            ],
            Some("page-2"),
        ),
    );
    pages.insert(
        Some("page-2".to_string()),
        page_json(
            vec![tx_json(103, "hash-a3", ADDRESS_A_KEY, "SUCCESS")],
            None,
        ),
    );
    let url = start_mock_server(pages);
    add_mock_profile(dir.path(), &url);
    let address_a = address_for(ADDRESS_A_KEY);

    let output = run_transactions(
        dir.path(),
        &[&address_a, "--start-ledger", "100", "--max-matches", "1"],
    );
    assert!(output.status.success(), "Command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Matched: 1"));
    assert!(stdout.contains("Pages: 1"));
    assert!(stdout.contains("hash-a1"));
    assert!(!stdout.contains("hash-a2"));
    assert!(!stdout.contains("hash-a3"));
    assert!(stdout.contains("Next cursor: page-2"));
}

#[test]
fn max_pages_fetches_exactly_one_page() {
    let dir = tempdir().unwrap();
    let url = start_mock_server(two_page_fixture());
    add_mock_profile(dir.path(), &url);
    let address_a = address_for(ADDRESS_A_KEY);

    let output = run_transactions(
        dir.path(),
        &[&address_a, "--start-ledger", "100", "--max-pages", "1"],
    );
    assert!(output.status.success(), "Command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Pages: 1"));
    assert!(stdout.contains("Next cursor: page-2"));
}

#[test]
fn limit_out_of_range_is_a_usage_error() {
    let dir = tempdir().unwrap();
    let url = start_mock_server(two_page_fixture());
    add_mock_profile(dir.path(), &url);
    let address_a = address_for(ADDRESS_A_KEY);

    for limit in ["0", "201"] {
        let output = run_transactions(
            dir.path(),
            &[&address_a, "--start-ledger", "100", "--limit", limit],
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "limit {limit} must be a usage error (exit 2)"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains('1'),
            "error names the lower bound: {stderr}"
        );
        assert!(
            stderr.contains("200"),
            "error names the upper bound: {stderr}"
        );
    }
}

#[test]
fn cursor_and_start_ledger_together_is_a_usage_error() {
    let dir = tempdir().unwrap();
    let url = start_mock_server(two_page_fixture());
    add_mock_profile(dir.path(), &url);
    let address_a = address_for(ADDRESS_A_KEY);

    let output = run_transactions(
        dir.path(),
        &[&address_a, "--start-ledger", "100", "--cursor", "abc"],
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "must be a usage error (exit 2)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cursor"),
        "error names the cursor: {stderr}"
    );
    assert!(
        stderr.contains("start-ledger"),
        "error names start-ledger: {stderr}"
    );
}

#[test]
fn empty_scan_exits_zero_with_summary() {
    let dir = tempdir().unwrap();
    let mut pages = HashMap::new();
    pages.insert(None, page_json(vec![], None));
    let url = start_mock_server(pages);
    add_mock_profile(dir.path(), &url);
    let address_a = address_for(ADDRESS_A_KEY);

    let output = run_transactions(dir.path(), &[&address_a, "--start-ledger", "100"]);
    assert!(output.status.success(), "Empty scan must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("no transactions"));
    assert!(stdout.contains("Scanned: 0"));
    assert!(stdout.contains("Matched: 0"));
    assert!(stdout.contains("oldestLedger 1"));
    assert!(stdout.contains("latestLedger 500"));
}

#[test]
fn invalid_address_is_an_error() {
    let dir = tempdir().unwrap();
    let url = start_mock_server(two_page_fixture());
    add_mock_profile(dir.path(), &url);

    let output = run_transactions(dir.path(), &["NOT_AN_ADDRESS"]);
    assert!(!output.status.success(), "Invalid address must fail");
}
