use assert_cmd::Command;
use base64::Engine;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use stellar_xdr::{Limits, OperationBody, ReadXdr, TransactionEnvelope};
use tempfile::tempdir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = sdkt();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

const VALID_CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

/// LedgerEntry XDR: an account entry with seq_num 41 (next sequence 42).
const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";

/// SorobanTransactionData XDR with a 150 stroop resource fee.
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

/// Read one HTTP request in full: headers, then as many body bytes as
/// `Content-Length` declares, so a request split across reads is not truncated.
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

/// Everything the mock saw: each JSON-RPC method in order, and the body of the
/// `sendTransaction` request if one arrived.
#[derive(Default)]
struct Seen {
    methods: Vec<String>,
    submitted: Option<String>,
}

/// Mock JSON-RPC server for the extend flow, routed by method name.
///
/// `getLatestLedger` reports `latest_ledger`, so a test can show that the
/// current ledger is not added to `--ledgers`.
fn mock_extend_rpc(latest_ledger: u32) -> (String, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Seen>> = Default::default();
    let seen_thread = seen.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_request(&mut sock);
            let body = req.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
            let method = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v["method"].as_str().map(str::to_string))
                .unwrap_or_default();
            {
                let mut s = seen_thread.lock().unwrap();
                s.methods.push(method.clone());
                if method == "sendTransaction" {
                    s.submitted = Some(body.to_string());
                }
            }

            let resp_body = match method.as_str() {
                "getLatestLedger" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"id":"mock","protocolVersion":22,"sequence":{latest_ledger}}}}}"#
                ),
                "getLedgerEntries" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":{latest_ledger}}}}}"#
                ),
                "simulateTransaction" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[],"latestLedger":"{latest_ledger}","events":[]}}}}"#
                ),
                "sendTransaction" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeefcafe","status":"PENDING","latestLedger":"100"}}"#.to_string()
                }
                "getTransaction" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg=="}}"#.to_string()
                }
                _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string(),
            };

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    (url, seen)
}

/// Decode the `extendTo` of the `ExtendFootprintTtl` operation in a
/// `sendTransaction` request body.
fn submitted_extend_to(send_body: &str) -> u32 {
    let request: serde_json::Value = serde_json::from_str(send_body).unwrap();
    let envelope = request["params"]["transaction"]
        .as_str()
        .unwrap_or_else(|| panic!("no transaction in sendTransaction: {send_body}"));
    let raw = base64::engine::general_purpose::STANDARD
        .decode(envelope)
        .unwrap();
    let TransactionEnvelope::Tx(v1) = TransactionEnvelope::from_xdr(raw, Limits::none()).unwrap()
    else {
        panic!("expected a v1 transaction envelope");
    };
    match &v1.tx.operations[0].body {
        OperationBody::ExtendFootprintTtl(op) => op.extend_to,
        other => panic!("expected ExtendFootprintTtl, got {other:?}"),
    }
}

/// Register the mock as a network profile and create an identity to sign with.
fn setup_mock_network(dir: &std::path::Path, rpc_url: &str) {
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
    sdkt_isolated(dir)
        .args(["identity", "generate", "alice"])
        .assert()
        .success();
}

// ---------- #110: --ledgers is a relative TTL ----------

/// `ExtendFootprintTTLOp.extendTo` is relative: "extend the TTL of the entries
/// ... so they will live at least extendTo ledgers from lcl"
/// (Stellar-transaction.x). `--ledgers` must therefore reach the operation
/// unchanged. Adding the current ledger would ask for ~800k extra ledgers here,
/// and on a mature network would always exceed the maximum entry TTL.
#[test]
fn storage_extend_passes_ledgers_to_the_operation_unchanged() {
    let dir = tempdir().unwrap();
    let (url, seen) = mock_extend_rpc(800_000);
    setup_mock_network(dir.path(), &url);

    let out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "extend",
            "--contract",
            VALID_CONTRACT,
            "--ledgers",
            "1000",
            "--identity",
            "alice",
            "--format",
            "json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("Invalid JSON: {e}\n{stdout}"));
    assert_eq!(parsed["extend_to"], 1_000, "{stdout}");

    // The value that reaches the network is the one that matters.
    let submitted = seen.lock().unwrap().submitted.clone();
    let submitted = submitted.expect("a transaction should have been submitted");
    assert_eq!(submitted_extend_to(&submitted), 1_000);
}

#[test]
fn storage_extend_help_shows_contract_and_ledgers() {
    sdkt()
        .args(["storage", "extend", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--contract"))
        .stdout(predicate::str::contains("--ledgers"))
        .stdout(predicate::str::contains("--key"))
        .stdout(predicate::str::contains("--identity"));
}

#[test]
fn storage_extend_rejects_missing_ledgers() {
    sdkt()
        .args([
            "storage",
            "extend",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
        ])
        .assert()
        .failure();
}

#[test]
fn storage_extend_rejects_zero_ledgers_offline() {
    sdkt()
        .args([
            "storage",
            "extend",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            "--ledgers",
            "0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("greater than 0"));
}

#[test]
fn storage_extend_rejects_empty_contract_offline() {
    sdkt()
        .args(["storage", "extend", "--contract", "", "--ledgers", "1000"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not be empty"));
}

#[test]
fn storage_extend_rejects_invalid_ledger_key_offline() {
    sdkt()
        .args([
            "storage",
            "extend",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            "--ledgers",
            "1000",
            "--key",
            "not-a-valid-key",
        ])
        .assert()
        .failure();
}

#[test]
fn storage_read_help_shows_contract_and_key_xdr() {
    sdkt()
        .args(["storage", "read", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--contract"))
        .stdout(predicate::str::contains("--key-xdr"))
        .stdout(predicate::str::contains("--abi"))
        .stdout(predicate::str::contains("--format"));
}

#[test]
fn storage_read_rejects_missing_key_xdr_offline() {
    sdkt()
        .args([
            "storage",
            "read",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
        ])
        .assert()
        .failure();
}

#[test]
fn storage_read_rejects_empty_contract_offline() {
    sdkt()
        .args([
            "storage",
            "read",
            "--contract",
            "",
            "--key-xdr",
            "AAAABQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not be empty"));
}

#[test]
fn storage_read_rejects_empty_key_xdr_offline() {
    sdkt()
        .args([
            "storage",
            "read",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            "--key-xdr",
            "",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not be empty"));
}

#[test]
fn storage_read_rejects_invalid_base64_offline() {
    sdkt()
        .args([
            "storage",
            "read",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            "--key-xdr",
            "not-valid-base64!!!",
        ])
        .assert()
        .failure();
}
