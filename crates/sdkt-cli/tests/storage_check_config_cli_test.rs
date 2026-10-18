//! CLI regression tests for `[storage]` config wiring on the `sdkt storage
//! check` path (issue #375, reviewer round 3). Offline and deterministic: a
//! mock JSON-RPC server records each `getLedgerEntries` request and reports a
//! fixed live-until ledger, so the test can prove both settings are honoured:
//!
//! * `storage.max_entries` — the request's `keys` array is chunked to at most
//!   `max_entries` keys per batch (follow-up batches included).
//! * `storage.ttl_warning_days` — the entry's `expiring_soon` flag flips when
//!   the configured day threshold is crossed.
//!
//! The mock reports `liveUntilLedgerSeq` such that the remaining TTL is
//! exactly 2 days (34_560 ledgers at 5 s/ledger), so `ttl_warning_days = 1`
//! marks it expiring-soon and `ttl_warning_days = 5` does not.

use assert_cmd::Command;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

const CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

/// LedgerKey XDR (base64) of a persistent ContractData entry with the given
/// `ScVal::U32` index. Built at test time from `stellar_xdr` so it always
/// matches the decoder, and each index yields a distinct key (so footprint
/// de-duplication leaves enough keys to exercise `max_entries` batching).
fn data_key_xdr(index: u32) -> String {
    use stellar_xdr::{
        ContractDataDurability, ContractId, Hash, LedgerKey, LedgerKeyContractData, Limits,
        ScAddress, ScVal, WriteXdr,
    };
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(ContractId(Hash([9; 32]))),
        key: ScVal::U32(index),
        durability: ContractDataDurability::Persistent,
    });
    base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        key.to_xdr(Limits::none()).unwrap(),
    )
}

/// Live-until ledger reported by the mock: current ledger 1_000_000 + 2 days.
const LIVE_UNTIL: u32 = 1_000_000 + 34_560;
const CURRENT_LEDGER: u32 = 1_000_000;

#[derive(Default)]
struct Seen {
    /// Number of keys in every `getLedgerEntries` request, in order.
    batch_sizes: Vec<usize>,
}

fn read_request(sock: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
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

/// Mock RPC that answers `getLatestLedger` and `getLedgerEntries`.
///
/// Each `getLedgerEntries` response returns one entry per requested key so the
/// CLI's batch loop is exercised (more keys than `max_entries` → follow-up
/// requests). `liveUntilLedgerSeq` is fixed at `LIVE_UNTIL`.
fn mock_storage_check_rpc() -> (String, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Seen>> = Default::default();
    let seen_thread = seen.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let req = read_request(&mut sock);
            let body = req.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
            let json: serde_json::Value =
                serde_json::from_str(body).unwrap_or_else(|_| serde_json::json!({}));
            let method = json["method"].as_str().unwrap_or("").to_string();

            let resp_body = match method.as_str() {
                "getLatestLedger" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"id":"mock","protocolVersion":22,"sequence":{CURRENT_LEDGER}}}}}"#
                ),
                "getLedgerEntries" => {
                    let keys = json["params"]["keys"].as_array().cloned().unwrap_or_default();
                    seen_thread.lock().unwrap().batch_sizes.push(keys.len());
                    let entries: Vec<serde_json::Value> = keys
                        .iter()
                        .map(|k| {
                            serde_json::json!({
                                "key": k.as_str().unwrap_or(""),
                                "xdr": "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=",
                                "lastModifiedLedgerSeq": 1,
                                "liveUntilLedgerSeq": LIVE_UNTIL
                            })
                        })
                        .collect();
                    let result = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "entries": entries,
                            "latestLedger": CURRENT_LEDGER
                        }
                    });
                    result.to_string()
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

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env_remove("HOME");
    cmd
}

/// `storage.max_entries = 2` must chunk the request: with the instance key
/// plus 3 `--key-xdr` entries the CLI issues two `getLedgerEntries` requests
/// (2 keys, then 2 keys) instead of one 4-key request.
#[test]
fn cli_storage_check_honours_max_entries() {
    let (rpc_url, seen) = mock_storage_check_rpc();
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(".sdkt.toml"),
        "[storage]\nmax_entries = 2\nttl_warning_days = 5\n",
    )
    .unwrap();

    let keys = [data_key_xdr(7), data_key_xdr(8), data_key_xdr(9)];
    let key_args: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
    sdkt()
        .current_dir(dir.path())
        .args([
            "storage",
            "check",
            CONTRACT_ID,
            "--key-xdr",
            key_args[0],
            "--key-xdr",
            key_args[1],
            "--key-xdr",
            key_args[2],
            "--rpc-url",
            &rpc_url,
            "--format",
            "json",
        ])
        .assert()
        .success();

    let batches = seen.lock().unwrap().batch_sizes.clone();
    assert!(
        batches.iter().all(|&n| n <= 2),
        "every getLedgerEntries request must respect max_entries = 2, got batches: {batches:?}"
    );
    assert!(
        batches.len() >= 2,
        "4 keys with max_entries = 2 must produce at least 2 batches, got: {batches:?}"
    );
}

/// `storage.ttl_warning_days` must drive the `expiring_soon` flag: the mock
/// entry has exactly 2 days of TTL left, so a 1-day threshold marks it
/// expiring-soon while a 5-day threshold does not.
#[test]
fn cli_storage_check_honours_ttl_warning_days() {
    let (rpc_url, _seen) = mock_storage_check_rpc();

    // 1-day threshold → 2 days of TTL remaining is NOT expiring-soon
    // (the remaining window exceeds the warning window).
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(".sdkt.toml"),
        "[storage]\nmax_entries = 200\nttl_warning_days = 1\n",
    )
    .unwrap();
    let out = sdkt()
        .current_dir(dir.path())
        .args([
            "storage",
            "check",
            CONTRACT_ID,
            "--rpc-url",
            &rpc_url,
            "--format",
            "json",
        ])
        .output()
        .expect("storage check runs");
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("storage check prints JSON");
    assert_eq!(
        json["entries"][0]["expiring_soon"],
        serde_json::json!(false),
        "2 days of TTL must not be expiring_soon when ttl_warning_days = 1, got: {json}"
    );

    // 5-day threshold → same entry IS expiring-soon (2 days of TTL left is
    // inside the 5-day warning window).
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(".sdkt.toml"),
        "[storage]\nmax_entries = 200\nttl_warning_days = 5\n",
    )
    .unwrap();
    let out = sdkt()
        .current_dir(dir.path())
        .args([
            "storage",
            "check",
            CONTRACT_ID,
            "--rpc-url",
            &rpc_url,
            "--format",
            "json",
        ])
        .output()
        .expect("storage check runs");
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("storage check prints JSON");
    assert_eq!(
        json["entries"][0]["expiring_soon"],
        serde_json::json!(true),
        "2 days of TTL must be expiring_soon when ttl_warning_days = 5, got: {json}"
    );
}
