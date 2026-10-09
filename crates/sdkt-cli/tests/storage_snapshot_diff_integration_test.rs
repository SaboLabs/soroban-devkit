use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
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

/// Read one HTTP request in full (headers + body according to Content-Length).
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

/// Encode a `ContractData` `LedgerEntry` carrying `val` for the given key.
fn contract_data_entry_xdr(key: &str, val: stellar_xdr::ScVal) -> String {
    use stellar_xdr::WriteXdr;

    let ledger_key = sdkt_xdr::decode_ledger_key(key).expect("valid ledger key");
    let (contract, durability, sc_key) = match ledger_key {
        stellar_xdr::LedgerKey::ContractData(cd) => (cd.contract, cd.durability, cd.key),
        other => panic!("expected ContractData ledger key, got {other:?}"),
    };

    let ledger_entry = stellar_xdr::LedgerEntry {
        last_modified_ledger_seq: 100,
        data: stellar_xdr::LedgerEntryData::ContractData(stellar_xdr::ContractDataEntry {
            ext: stellar_xdr::ExtensionPoint::V0,
            contract,
            key: sc_key,
            durability,
            val,
        }),
        ext: stellar_xdr::LedgerEntryExt::V0,
    };

    let mut buf = Vec::new();
    let mut limited = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    ledger_entry.write_xdr(&mut limited).unwrap();
    base64_encode(&buf)
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Shared mutable state for the mock RPC: an override map from ledger key to
/// the `ScVal::U32` value the server returns for that key. Keys absent from the
/// map get [`DEFAULT_MOCK_VALUE`], so only an explicitly overridden key can
/// change between two captures.
type ValueCell = Arc<Mutex<std::collections::HashMap<String, u32>>>;

/// Value returned for every key not present in the override map.
const DEFAULT_MOCK_VALUE: u32 = 7;

/// Mock JSON-RPC server whose entry values come from `values` and whose TTL and
/// rent are held fixed, so the only thing that can differ between two captures
/// is a captured value the test deliberately changed.
fn mock_snapshot_rpc(latest_ledger: u32, values: ValueCell) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_request(&mut sock);
            let body = req.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
            let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
            let method = parsed
                .as_ref()
                .and_then(|v| v["method"].as_str())
                .unwrap_or_default()
                .to_string();

            let resp_body = match method.as_str() {
                "getLatestLedger" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"id":"mock","protocolVersion":22,"sequence":{latest_ledger}}}}}"#
                ),
                "getLedgerEntries" => {
                    let keys: Vec<String> = parsed
                        .as_ref()
                        .and_then(|v| v["params"]["keys"].as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|k| k.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let overrides = values.lock().unwrap();

                    let entries: Vec<serde_json::Value> = keys
                        .iter()
                        .map(|k| {
                            let current = *overrides.get(k).unwrap_or(&DEFAULT_MOCK_VALUE);
                            serde_json::json!({
                                "key": k,
                                // Fixed TTL for every entry: only the value differs.
                                "xdr": contract_data_entry_xdr(k, stellar_xdr::ScVal::U32(current)),
                                "lastModifiedLedgerSeq": 1000,
                                "liveUntilLedgerSeq": latest_ledger + 20000,
                            })
                        })
                        .collect();

                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": { "entries": entries, "latestLedger": latest_ledger }
                    })
                    .to_string()
                }
                _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not found"}}"#
                    .to_string(),
            };

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    url
}

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
}

fn contract_data_key(key: stellar_xdr::ScVal) -> String {
    sdkt_xdr::encode_ledger_key(&sdkt_xdr::LedgerKeyParams::ContractDataEntry {
        contract: VALID_CONTRACT.to_string(),
        key,
        durability: stellar_xdr::ContractDataDurability::Persistent,
    })
    .unwrap()
}

// ---------------- Help / argument tests ----------------

#[test]
fn storage_snapshot_help_documents_value_capture() {
    sdkt()
        .args(["storage", "snapshot", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CONTRACT_ID"))
        .stdout(predicate::str::contains("--out"))
        .stdout(predicate::str::contains("--key-xdr"));
}

#[test]
fn storage_diff_help_documents_value_delta() {
    sdkt()
        .args(["storage", "diff", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("value_changed"));
}

#[test]
fn storage_diff_missing_snapshot_file_errors_offline() {
    sdkt()
        .args(["storage", "diff", "/nonexistent/snapshot.json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read snapshot"));
}

#[test]
fn storage_snapshot_rejects_invalid_key_xdr_offline() {
    sdkt()
        .args([
            "storage",
            "snapshot",
            VALID_CONTRACT,
            "--key-xdr",
            "not-a-valid-ledger-key",
            "--out",
            "/tmp/should-not-be-written.json",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid LedgerKey"));
}

// ---------------- Mock RPC integration tests ----------------

#[test]
fn storage_snapshot_then_diff_reports_value_only_change() {
    let dir = tempdir().unwrap();
    let key = contract_data_key(stellar_xdr::ScVal::U32(10));

    let value: ValueCell = Arc::new(Mutex::new(std::collections::HashMap::new()));
    value.lock().unwrap().insert(key.clone(), 100);
    let url = mock_snapshot_rpc(1200, value.clone());
    setup_mock_network(dir.path(), &url);

    let snapshot_path = dir.path().join("snapshot.json");
    let snapshot_path_str = snapshot_path.to_string_lossy().to_string();

    // 1. Capture the base snapshot with value 100.
    let out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "snapshot",
            VALID_CONTRACT,
            "--key-xdr",
            &key,
            "--out",
            &snapshot_path_str,
            "--format",
            "json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    assert!(
        stdout.contains("\"value\""),
        "snapshot document must record a per-entry value: {stdout}"
    );

    let document: serde_json::Value = serde_json::from_str(&stdout).expect("valid snapshot json");
    let entries = document["entries"].as_array().expect("entries array");
    let tracked = entries
        .iter()
        .find(|e| e["key"] == key)
        .expect("tracked key present in snapshot");
    assert!(
        tracked["value"].is_string(),
        "tracked entry must carry a captured value: {tracked}"
    );
    let captured_before = tracked["value"].as_str().unwrap().to_string();

    // Also verify the file on disk holds the same document.
    let on_disk = std::fs::read_to_string(&snapshot_path).expect("snapshot file written");
    let disk_doc: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
    assert_eq!(disk_doc["entries"], document["entries"]);

    // 2. Only the tracked key's stored value changes; TTL stays fixed and the
    // instance entry is left untouched.
    *value.lock().unwrap().get_mut(&key).unwrap() = 0;

    let diff_out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "diff",
            &snapshot_path_str,
            "--format",
            "json",
        ])
        .assert()
        .success();

    let diff_stdout = String::from_utf8_lossy(&diff_out.get_output().stdout).to_string();
    let diff: serde_json::Value = serde_json::from_str(&diff_stdout).expect("valid diff json");

    let value_changed = diff["value_changed"]
        .as_array()
        .unwrap_or_else(|| panic!("value_changed must be present: {diff_stdout}"));
    assert_eq!(value_changed.len(), 1, "{diff_stdout}");
    assert_eq!(value_changed[0]["key"], key);
    assert_eq!(value_changed[0]["before"], captured_before);
    assert!(value_changed[0]["after"].is_string());

    // TTL and rent were fixed, so only the value delta exists.
    assert!(
        diff.get("ttl_changed").is_none(),
        "a value-only change must not report TTL deltas: {diff_stdout}"
    );
    assert_eq!(
        diff["unchanged"], 1,
        "the instance entry is unchanged: {diff_stdout}"
    );
    assert_eq!(diff["contract_id"], VALID_CONTRACT);
}

#[test]
fn storage_diff_pretty_reports_value_changed_section() {
    let dir = tempdir().unwrap();
    let key = contract_data_key(stellar_xdr::ScVal::U32(10));

    let value: ValueCell = Arc::new(Mutex::new(std::collections::HashMap::new()));
    value.lock().unwrap().insert(key.clone(), 55);
    let url = mock_snapshot_rpc(2000, value.clone());
    setup_mock_network(dir.path(), &url);

    let snapshot_path = dir.path().join("snapshot.json");
    let snapshot_path_str = snapshot_path.to_string_lossy().to_string();

    sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "snapshot",
            VALID_CONTRACT,
            "--key-xdr",
            &key,
            "--out",
            &snapshot_path_str,
        ])
        .assert()
        .success();

    *value.lock().unwrap().get_mut(&key).unwrap() = 77;

    let out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "diff",
            &snapshot_path_str,
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    assert!(stdout.contains("Changed:   1"), "{stdout}");
    assert!(stdout.contains("Unchanged: 1"), "{stdout}");
    assert!(stdout.contains("Value Changed:"), "{stdout}");
    assert!(stdout.contains(&key), "{stdout}");
}

#[test]
fn storage_diff_unchanged_when_nothing_moves() {
    let dir = tempdir().unwrap();
    let key = contract_data_key(stellar_xdr::ScVal::U32(10));

    let value: ValueCell = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let url = mock_snapshot_rpc(3000, value.clone());
    setup_mock_network(dir.path(), &url);

    let snapshot_path = dir.path().join("snapshot.json");
    let snapshot_path_str = snapshot_path.to_string_lossy().to_string();

    sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "snapshot",
            VALID_CONTRACT,
            "--key-xdr",
            &key,
            "--out",
            &snapshot_path_str,
            "--format",
            "json",
        ])
        .assert()
        .success();

    // No mutation: value and TTL both match.
    let out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "diff",
            &snapshot_path_str,
            "--format",
            "json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let diff: serde_json::Value = serde_json::from_str(&stdout).expect("valid diff json");
    assert!(
        diff.get("value_changed").is_none(),
        "no value may change: {stdout}"
    );
    assert!(
        diff.get("ttl_changed").is_none(),
        "no TTL may change: {stdout}"
    );
    assert_eq!(diff["unchanged"], 2, "{stdout}");
}

#[test]
fn diff_accepts_legacy_snapshot_without_values() {
    let dir = tempdir().unwrap();
    let key = contract_data_key(stellar_xdr::ScVal::U32(10));

    let value: ValueCell = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let url = mock_snapshot_rpc(4000, value.clone());
    setup_mock_network(dir.path(), &url);

    // A snapshot document written before value capture existed: no `value` key.
    let legacy = serde_json::json!({
        "contract_id": VALID_CONTRACT,
        "entries": [
            {
                "key": key,
                "class": "persistent",
                "durability": "persistent",
                "current_ttl": 20000,
                "extension_cost_stroops": 2000000
            }
        ]
    });
    let snapshot_path = dir.path().join("legacy.json");
    std::fs::write(
        &snapshot_path,
        serde_json::to_string_pretty(&legacy).unwrap(),
    )
    .unwrap();

    let out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "diff",
            &snapshot_path.to_string_lossy(),
            "--format",
            "json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let diff: serde_json::Value = serde_json::from_str(&stdout).expect("valid diff json");

    // Value capture was absent in the base document, so there is nothing to
    // compare; the TTL also matches (20000), so the entry is unchanged.
    assert!(diff.get("value_changed").is_none(), "{stdout}");
    assert!(diff.get("ttl_changed").is_none(), "{stdout}");
    assert_eq!(diff["unchanged"], 1, "{stdout}");
}

#[test]
fn storage_diff_rejects_contract_override_that_mismatches_snapshot() {
    let dir = tempdir().unwrap();
    let key = contract_data_key(stellar_xdr::ScVal::U32(10));

    // A snapshot recorded for VALID_CONTRACT: its ledger keys embed that
    // contract's address, so diffing another contract's live storage against it
    // would mix two contracts under one header.
    let snapshot = serde_json::json!({
        "contract_id": VALID_CONTRACT,
        "entries": [
            {
                "key": key,
                "class": "persistent",
                "durability": "persistent",
                "current_ttl": 20000,
                "extension_cost_stroops": 2000000
            }
        ]
    });
    let snapshot_path = dir.path().join("snapshot.json");
    std::fs::write(
        &snapshot_path,
        serde_json::to_string_pretty(&snapshot).unwrap(),
    )
    .unwrap();

    // Different, well-formed StrKey contract id.
    let other = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

    sdkt()
        .args([
            "storage",
            "diff",
            &snapshot_path.to_string_lossy(),
            "--contract",
            other,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "does not match the snapshot's contract",
        ));
}

#[test]
fn storage_diff_extra_keys_report_added_keys() {
    let dir = tempdir().unwrap();
    let tracked = contract_data_key(stellar_xdr::ScVal::U32(10));
    let created_after = contract_data_key(stellar_xdr::ScVal::U32(11));

    let value: ValueCell = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let url = mock_snapshot_rpc(1500, value.clone());
    setup_mock_network(dir.path(), &url);

    let snapshot_path = dir.path().join("snapshot.json");
    let snapshot_path_str = snapshot_path.to_string_lossy().to_string();

    sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "snapshot",
            VALID_CONTRACT,
            "--key-xdr",
            &tracked,
            "--out",
            &snapshot_path_str,
            "--format",
            "json",
        ])
        .assert()
        .success();

    // A key created after the snapshot is invisible to a bare `diff` (RPC only
    // returns the keys that were requested), so it must be probed explicitly.
    let out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "diff",
            &snapshot_path_str,
            "--key-xdr",
            &created_after,
            "--format",
            "json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let diff: serde_json::Value = serde_json::from_str(&stdout).expect("valid diff json");
    let added = diff["added"]
        .as_array()
        .unwrap_or_else(|| panic!("added must be present when a probed key is new: {stdout}"));
    assert!(
        added.iter().any(|k| k == &serde_json::json!(created_after)),
        "probed key must be reported as added: {stdout}"
    );
}
