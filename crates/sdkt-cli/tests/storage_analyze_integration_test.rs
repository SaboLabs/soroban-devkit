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

#[derive(Default)]
struct SeenRequests {
    methods: Vec<String>,
    keys_queried: Vec<Vec<String>>,
}

/// Mock JSON-RPC server returning predefined storage entries for getLedgerEntries.
fn mock_analyze_rpc(
    latest_ledger: u32,
    entries_json: serde_json::Value,
) -> (String, Arc<Mutex<SeenRequests>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<SeenRequests>> = Default::default();
    let seen_thread = seen.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_request(&mut sock);
            let body = req.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
            let parsed_req = serde_json::from_str::<serde_json::Value>(body).ok();
            let method = parsed_req
                .as_ref()
                .and_then(|v| v["method"].as_str().map(str::to_string))
                .unwrap_or_default();

            {
                let mut s = seen_thread.lock().unwrap();
                s.methods.push(method.clone());
                if method == "getLedgerEntries" {
                    if let Some(keys) = parsed_req
                        .as_ref()
                        .and_then(|v| v["params"]["keys"].as_array())
                    {
                        let k_strs = keys
                            .iter()
                            .filter_map(|k| k.as_str().map(str::to_string))
                            .collect();
                        s.keys_queried.push(k_strs);
                    }
                }
            }

            let resp_body = match method.as_str() {
                "getLatestLedger" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"id":"mock","protocolVersion":22,"sequence":{latest_ledger}}}}}"#
                ),
                "getLedgerEntries" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "entries": entries_json,
                        "latestLedger": latest_ledger
                    }
                })
                .to_string(),
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

    (url, seen)
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

/// Helper to encode a ContractData LedgerKey
fn contract_data_key(
    contract: &str,
    key: stellar_xdr::ScVal,
    durability: stellar_xdr::ContractDataDurability,
) -> String {
    sdkt_xdr::encode_ledger_key(&sdkt_xdr::LedgerKeyParams::ContractDataEntry {
        contract: contract.to_string(),
        key,
        durability,
    })
    .unwrap()
}

// ---------------- Existing tests ----------------

/// `sdkt storage analyze <id>` should reject an empty contract id with a
/// non-zero exit and an error message, without contacting the network.
#[test]
fn storage_analyze_empty_id_errors() {
    sdkt()
        .args(["storage", "analyze", ""])
        .assert()
        .failure()
        .stderr(predicates::str::contains("Error analyzing storage"));
}

/// `sdkt storage analyze --help` should list the subcommand and document the
/// Instance/Persistent/Temporary categorization without erroring.
#[test]
fn storage_analyze_help_documents_categorization() {
    sdkt()
        .args(["storage", "analyze", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("CONTRACT_ID"))
        .stdout(predicates::str::contains("Instance/Persistent/Temporary"));
}

/// `--format json` is accepted as a valid flag and does not crash at parse time.
#[test]
fn storage_analyze_accepts_json_format_flag() {
    sdkt()
        .args([
            "storage",
            "analyze",
            "CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
            "--format",
            "json",
        ])
        .assert()
        // Network call will fail (no RPC), but the flag parses and we reach execution.
        .failure();
}

// ---------------- Acceptance criteria & CLI argument tests ----------------

#[test]
fn storage_analyze_help_shows_explicit_key_flags() {
    sdkt()
        .args(["storage", "analyze", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--key-xdr"))
        .stdout(predicate::str::contains("--map-key"))
        .stdout(predicate::str::contains("--key-arg"))
        .stdout(predicate::str::contains("--durability"));
}

#[test]
fn storage_analyze_rejects_key_arg_without_map_key_offline() {
    sdkt()
        .args(["storage", "analyze", VALID_CONTRACT, "--key-arg", "u32:1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--key-arg requires --map-key"));
}

#[test]
fn storage_analyze_rejects_empty_key_xdr_offline() {
    sdkt()
        .args(["storage", "analyze", VALID_CONTRACT, "--key-xdr", ""])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--key-xdr must not be empty"));
}

#[test]
fn storage_analyze_rejects_invalid_key_xdr_offline() {
    sdkt()
        .args([
            "storage",
            "analyze",
            VALID_CONTRACT,
            "--key-xdr",
            "not-a-valid-ledger-key",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid LedgerKey"));
}

#[test]
fn storage_analyze_rejects_unknown_key_arg_type_offline() {
    sdkt()
        .args([
            "storage",
            "analyze",
            VALID_CONTRACT,
            "--map-key",
            "balances",
            "--key-arg",
            "weird:1",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown arg type"));
}

#[test]
fn storage_analyze_rejects_invalid_durability_offline() {
    sdkt()
        .args([
            "storage",
            "analyze",
            VALID_CONTRACT,
            "--map-key",
            "balances",
            "--durability",
            "forever",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid durability"));
}

// ---------------- Mock RPC integration tests ----------------

#[test]
fn storage_analyze_reports_persistent_and_temporary_keys_with_ttl_rows() {
    let dir = tempdir().unwrap();

    let persistent_key = contract_data_key(
        VALID_CONTRACT,
        stellar_xdr::ScVal::U32(10),
        stellar_xdr::ContractDataDurability::Persistent,
    );
    let temporary_key = contract_data_key(
        VALID_CONTRACT,
        stellar_xdr::ScVal::U32(20),
        stellar_xdr::ContractDataDurability::Temporary,
    );
    let instance_key = contract_data_key(
        VALID_CONTRACT,
        stellar_xdr::ScVal::LedgerKeyContractInstance,
        stellar_xdr::ContractDataDurability::Persistent,
    );

    // Mock RPC returning instance entry, persistent entry, and temporary entry
    let entries = serde_json::json!([
        {
            "key": instance_key,
            "xdr": "AAAAAQAAAABpc25nAAAA",
            "lastModifiedLedgerSeq": 100,
            "liveUntilLedgerSeq": 200
        },
        {
            "key": persistent_key,
            "xdr": "AAAAAQAAAABpc25nAAAA",
            "lastModifiedLedgerSeq": 100,
            "liveUntilLedgerSeq": 300
        },
        {
            "key": temporary_key,
            "xdr": "AAAAAQAAAABpc25nAAAA",
            "lastModifiedLedgerSeq": 100,
            "liveUntilLedgerSeq": 400
        }
    ]);

    let (url, seen) = mock_analyze_rpc(100, entries);
    setup_mock_network(dir.path(), &url);

    // 1. JSON format verification
    let out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "analyze",
            VALID_CONTRACT,
            "--key-xdr",
            &persistent_key,
            "--key-xdr",
            &temporary_key,
            "--format",
            "json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let report: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("invalid json: {e}\n{stdout}"));

    assert_eq!(report["total_entries"], 3, "{stdout}");
    assert_eq!(report["instance_entries"], 1, "{stdout}");
    assert_eq!(report["persistent_entries"], 1, "{stdout}");
    assert_eq!(report["temporary_entries"], 1, "{stdout}");
    assert_eq!(report["other_entries"], 0, "{stdout}");

    let entries_arr = report["entries"].as_array().expect("entries array");
    assert_eq!(entries_arr.len(), 3);
    assert_eq!(entries_arr[0]["class"], "instance");
    assert_eq!(entries_arr[0]["current_ttl"], 100);
    assert_eq!(entries_arr[1]["class"], "persistent");
    assert_eq!(entries_arr[1]["current_ttl"], 200);
    assert_eq!(entries_arr[2]["class"], "temporary");
    assert_eq!(entries_arr[2]["current_ttl"], 300);

    let ttl_summary = &report["ttl_summary"];
    assert_eq!(ttl_summary["minimum_ttl"], 100);
    assert_eq!(ttl_summary["maximum_ttl"], 300);
    assert_eq!(ttl_summary["average_ttl"], 200);

    // Verify all 3 keys were queried
    let queried = seen.lock().unwrap().keys_queried.clone();
    assert!(!queried.is_empty());
    assert_eq!(queried[0].len(), 3);

    // 2. Pretty format verification
    let pretty_out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "analyze",
            VALID_CONTRACT,
            "--key-xdr",
            &persistent_key,
            "--key-xdr",
            &temporary_key,
        ])
        .assert()
        .success();

    let pretty_stdout = String::from_utf8_lossy(&pretty_out.get_output().stdout).to_string();
    assert!(pretty_stdout.contains("Total Entries: 3"));
    assert!(pretty_stdout.contains("Instance:    1"));
    assert!(pretty_stdout.contains("Persistent: 1"));
    assert!(pretty_stdout.contains("Temporary:   1"));
    assert!(pretty_stdout.contains("[instance] ttl=100"));
    assert!(pretty_stdout.contains("[persistent] ttl=200"));
    assert!(pretty_stdout.contains("[temporary] ttl=300"));
}

#[test]
fn storage_analyze_with_typed_key_and_raw_key() {
    let dir = tempdir().unwrap();

    let persistent_key = contract_data_key(
        VALID_CONTRACT,
        stellar_xdr::ScVal::U32(10),
        stellar_xdr::ContractDataDurability::Persistent,
    );
    // Typed temporary key for map-key "balances", arg "u32:100"
    let map_key =
        sdkt_xdr::build_map_key("balances", &["AAAAAQAAAAEAAAAEAAAAAAAAAGQ=".to_string()]).unwrap();
    let typed_temporary_key = contract_data_key(
        VALID_CONTRACT,
        map_key,
        stellar_xdr::ContractDataDurability::Temporary,
    );
    let instance_key = contract_data_key(
        VALID_CONTRACT,
        stellar_xdr::ScVal::LedgerKeyContractInstance,
        stellar_xdr::ContractDataDurability::Persistent,
    );

    let entries = serde_json::json!([
        {
            "key": instance_key,
            "xdr": "AAAAAQAAAABpc25nAAAA",
            "lastModifiedLedgerSeq": 100,
            "liveUntilLedgerSeq": 200
        },
        {
            "key": persistent_key,
            "xdr": "AAAAAQAAAABpc25nAAAA",
            "lastModifiedLedgerSeq": 100,
            "liveUntilLedgerSeq": 300
        },
        {
            "key": typed_temporary_key,
            "xdr": "AAAAAQAAAABpc25nAAAA",
            "lastModifiedLedgerSeq": 100,
            "liveUntilLedgerSeq": 400
        }
    ]);

    let (url, _seen) = mock_analyze_rpc(100, entries);
    setup_mock_network(dir.path(), &url);

    let out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "analyze",
            VALID_CONTRACT,
            "--key-xdr",
            &persistent_key,
            "--map-key",
            "balances",
            "--key-arg",
            "u32:100",
            "--durability",
            "temporary",
            "--format",
            "json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let report: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("invalid json: {e}\n{stdout}"));

    assert_eq!(report["total_entries"], 3);
    assert_eq!(report["instance_entries"], 1);
    assert_eq!(report["persistent_entries"], 1);
    assert_eq!(report["temporary_entries"], 1);
}

#[test]
fn storage_analyze_no_keys_invocation_reports_instance_only() {
    let dir = tempdir().unwrap();

    let instance_key = contract_data_key(
        VALID_CONTRACT,
        stellar_xdr::ScVal::LedgerKeyContractInstance,
        stellar_xdr::ContractDataDurability::Persistent,
    );

    let entries = serde_json::json!([
        {
            "key": instance_key,
            "xdr": "AAAAAQAAAABpc25nAAAA",
            "lastModifiedLedgerSeq": 100,
            "liveUntilLedgerSeq": 200
        }
    ]);

    let (url, seen) = mock_analyze_rpc(100, entries);
    setup_mock_network(dir.path(), &url);

    let out = sdkt_isolated(dir.path())
        .args([
            "storage",
            "--network-profile",
            "mocknet",
            "analyze",
            VALID_CONTRACT,
            "--format",
            "json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let report: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("invalid json: {e}\n{stdout}"));

    assert_eq!(report["total_entries"], 1);
    assert_eq!(report["instance_entries"], 1);
    assert_eq!(report["persistent_entries"], 0);
    assert_eq!(report["temporary_entries"], 0);

    // Verify only 1 key (the instance singleton) was queried
    let queried = seen.lock().unwrap().keys_queried.clone();
    assert_eq!(queried.len(), 1);
    assert_eq!(queried[0].len(), 1);
}
