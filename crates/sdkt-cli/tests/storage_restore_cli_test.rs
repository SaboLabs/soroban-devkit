//! Integration tests for `sdkt storage restore`.
//!
//! A mock JSON-RPC server returns a `restorePreamble` (or not) from
//! `simulateTransaction`, so the whole simulate -> adopt -> build -> sign ->
//! submit flow runs without a live network.

use assert_cmd::Command;
use base64::Engine;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use stellar_xdr::{
    ContractDataDurability, ContractId, Hash, LedgerFootprint, LedgerKey, LedgerKeyContractData,
    Limits, OperationBody, ReadXdr, ScAddress, ScVal, SorobanResources, SorobanTransactionData,
    SorobanTransactionDataExt, TransactionEnvelope, TransactionExt, VecM, WriteXdr,
};
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
const SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

/// LedgerEntry XDR: an account entry with seq_num 41 (next sequence 42).
const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";

/// SorobanTransactionData XDR with a 150 stroop resource fee (the invocation's own data).
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

fn b64(bytes: Vec<u8>) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Two archived persistent entries of `VALID_CONTRACT`: its instance and one data key.
fn archived_keys() -> Vec<LedgerKey> {
    let contract = ScAddress::Contract(ContractId(Hash(
        sdkt_xdr::decode_contract_id(VALID_CONTRACT).unwrap().0,
    )));
    vec![
        LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract.clone(),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        }),
        LedgerKey::ContractData(LedgerKeyContractData {
            contract,
            key: ScVal::U32(7),
            durability: ContractDataDurability::Persistent,
        }),
    ]
}

/// A preamble `SorobanTransactionData` whose read-write footprint is `keys`.
fn preamble_data(keys: Vec<LedgerKey>, resource_fee: i64) -> SorobanTransactionData {
    SorobanTransactionData {
        ext: SorobanTransactionDataExt::V0,
        resources: SorobanResources {
            footprint: LedgerFootprint {
                read_only: VecM::default(),
                read_write: VecM::try_from(keys).unwrap(),
            },
            instructions: 0,
            disk_read_bytes: 2_048,
            write_bytes: 2_048,
        },
        resource_fee,
    }
}

fn preamble_data_b64(data: &SorobanTransactionData) -> String {
    b64(data.to_xdr(Limits::none()).unwrap())
}

/// A valid invocation envelope to pass as `--envelope`.
fn invocation_envelope() -> String {
    sdkt_xdr::build_invoke_transaction(&sdkt_xdr::InvokeTransactionParams {
        source_account: SOURCE.into(),
        sequence: 1,
        fee: 100,
        contract_id: VALID_CONTRACT.into(),
        function: "get".into(),
        args: vec![],
    })
    .unwrap()
}

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
/// last `sendTransaction` request if one arrived.
#[derive(Default)]
struct Seen {
    methods: Vec<String>,
    submitted: Option<String>,
}

impl Seen {
    fn submitted_anything(&self) -> bool {
        self.methods.iter().any(|m| m == "sendTransaction")
    }
}

/// Mock JSON-RPC server for the restore flow. `restore_preamble` is the JSON
/// value of `restorePreamble` in the simulation result, or `None` to omit it.
fn mock_restore_rpc(restore_preamble: Option<serde_json::Value>) -> (String, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Seen>> = Default::default();
    let seen_thread = seen.clone();

    let mut simulate_result = serde_json::json!({
        "transactionData": SOROBAN_DATA_XDR,
        "minResourceFee": "150",
        "results": [],
        "latestLedger": "100",
        "events": [],
    });
    if let Some(preamble) = restore_preamble {
        simulate_result["restorePreamble"] = preamble;
    }
    let simulate_body =
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": simulate_result }).to_string();

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
                "getLedgerEntries" => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                ),
                "simulateTransaction" => simulate_body.clone(),
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

/// `sdkt storage --network-profile mocknet restore ...` plus `extra` args.
fn restore_cmd(dir: &std::path::Path, extra: &[&str]) -> Command {
    let envelope = invocation_envelope();
    let mut cmd = sdkt_isolated(dir);
    cmd.args([
        "storage",
        "--network-profile",
        "mocknet",
        "restore",
        "--contract",
        VALID_CONTRACT,
        "--envelope",
        envelope.as_str(),
        "--identity",
        "alice",
    ])
    .args(extra);
    cmd
}

/// Decode the submitted envelope: its operation must be `RestoreFootprint`.
/// Returns the transaction fee and its `SorobanTransactionData`.
fn submitted_restore(send_body: &str) -> (u32, SorobanTransactionData) {
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
    assert_eq!(v1.tx.operations.len(), 1);
    assert!(
        matches!(v1.tx.operations[0].body, OperationBody::RestoreFootprint(_)),
        "expected RestoreFootprint, got {:?}",
        v1.tx.operations[0].body
    );
    let TransactionExt::V1(data) = v1.tx.ext else {
        panic!("restore transaction must carry SorobanTransactionData");
    };
    (v1.tx.fee, data)
}

// ---------- Full restore flow ----------

#[test]
fn storage_restore_submits_restore_footprint_from_preamble() {
    let dir = tempdir().unwrap();
    let preamble = preamble_data(archived_keys(), 6_000);
    let (url, seen) = mock_restore_rpc(Some(serde_json::json!({
        "transactionData": preamble_data_b64(&preamble),
        "minResourceFee": "6000",
    })));
    setup_mock_network(dir.path(), &url);

    let out = restore_cmd(dir.path(), &["--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("Invalid JSON: {e}\n{stdout}"));
    assert_eq!(parsed["contract_id"], VALID_CONTRACT, "{stdout}");
    assert_eq!(parsed["hash"], "deadbeefcafe", "{stdout}");
    assert_eq!(parsed["status"], "SUCCESS", "{stdout}");
    assert_eq!(parsed["restored_keys"], 2, "{stdout}");
    assert_eq!(parsed["footprint_keys"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["min_resource_fee"], 6_000, "{stdout}");

    let submitted = seen.lock().unwrap().submitted.clone();
    let (fee, data) = submitted_restore(&submitted.expect("a restore should have been submitted"));
    assert_eq!(
        data.resources.footprint, preamble.resources.footprint,
        "the preamble footprint must reach the network unchanged"
    );
    assert!(
        data.resource_fee >= 6_000,
        "resource fee below minResourceFee"
    );
    assert!(fee >= 6_000, "transaction fee below minResourceFee");
    assert_eq!(
        parsed["fee"], fee,
        "reported fee must match the submitted fee"
    );
}

#[test]
fn storage_restore_raises_resource_fee_to_min_resource_fee() {
    let dir = tempdir().unwrap();
    // The preamble data under-declares its resource fee; minResourceFee is the floor.
    let preamble = preamble_data(archived_keys(), 1_000);
    let (url, seen) = mock_restore_rpc(Some(serde_json::json!({
        "transactionData": preamble_data_b64(&preamble),
        "minResourceFee": "7500",
    })));
    setup_mock_network(dir.path(), &url);

    restore_cmd(dir.path(), &["--format", "json"])
        .assert()
        .success();

    let submitted = seen.lock().unwrap().submitted.clone().unwrap();
    let (fee, data) = submitted_restore(&submitted);
    assert_eq!(data.resource_fee, 7_500);
    assert_eq!(fee, 7_600, "inclusion fee (100) + minResourceFee");
    assert_eq!(data.resources.footprint, preamble.resources.footprint);
}

#[test]
fn storage_restore_pretty_output_reports_hash_status_and_key_count() {
    let dir = tempdir().unwrap();
    let preamble = preamble_data(archived_keys(), 6_000);
    let (url, _seen) = mock_restore_rpc(Some(serde_json::json!({
        "transactionData": preamble_data_b64(&preamble),
        "minResourceFee": "6000",
    })));
    setup_mock_network(dir.path(), &url);

    restore_cmd(dir.path(), &[])
        .assert()
        .success()
        .stdout(predicate::str::contains("Storage Restore"))
        .stdout(predicate::str::contains("Restored Keys:  2"))
        .stdout(predicate::str::contains("TX Hash:        deadbeefcafe"))
        .stdout(predicate::str::contains("Status:         SUCCESS"));
}

// ---------- Dry run ----------

#[test]
fn storage_restore_dry_run_prints_keys_and_fee_without_submitting() {
    let dir = tempdir().unwrap();
    let preamble = preamble_data(archived_keys(), 6_000);
    let (url, seen) = mock_restore_rpc(Some(serde_json::json!({
        "transactionData": preamble_data_b64(&preamble),
        "minResourceFee": "6000",
    })));
    setup_mock_network(dir.path(), &url);

    let expected_keys: Vec<String> = archived_keys()
        .iter()
        .map(|k| b64(k.to_xdr(Limits::none()).unwrap()))
        .collect();

    restore_cmd(dir.path(), &["--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dry run, not submitted"))
        .stdout(predicate::str::contains("Restored Keys:  2"))
        .stdout(predicate::str::contains(expected_keys[0].as_str()))
        .stdout(predicate::str::contains(expected_keys[1].as_str()))
        .stdout(predicate::str::contains("6100 stroops"))
        .stdout(predicate::str::contains("Status:         DRY_RUN"));

    let seen = seen.lock().unwrap();
    assert!(
        !seen.submitted_anything(),
        "dry run must not submit: {:?}",
        seen.methods
    );
    assert!(!seen.methods.iter().any(|m| m == "getTransaction"));
}

#[test]
fn storage_restore_dry_run_json_has_no_hash() {
    let dir = tempdir().unwrap();
    let preamble = preamble_data(archived_keys(), 6_000);
    let (url, seen) = mock_restore_rpc(Some(serde_json::json!({
        "transactionData": preamble_data_b64(&preamble),
        "minResourceFee": "6000",
    })));
    setup_mock_network(dir.path(), &url);

    let out = restore_cmd(dir.path(), &["--dry-run", "--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("Invalid JSON: {e}\n{stdout}"));
    assert!(parsed["hash"].is_null(), "{stdout}");
    assert_eq!(parsed["status"], "DRY_RUN", "{stdout}");
    assert_eq!(parsed["restored_keys"], 2, "{stdout}");
    assert_eq!(parsed["fee"], 6_100, "{stdout}");
    assert!(!seen.lock().unwrap().submitted_anything());
}

// ---------- Error paths: nothing is submitted ----------

#[test]
fn storage_restore_without_preamble_reports_live_state() {
    let dir = tempdir().unwrap();
    let (url, seen) = mock_restore_rpc(None);
    setup_mock_network(dir.path(), &url);

    restore_cmd(dir.path(), &[])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Nothing to restore"))
        .stderr(predicate::str::contains("live"));
    assert!(!seen.lock().unwrap().submitted_anything());
}

#[test]
fn storage_restore_rejects_empty_preamble_footprint() {
    let dir = tempdir().unwrap();
    let preamble = preamble_data(vec![], 100);
    let (url, seen) = mock_restore_rpc(Some(serde_json::json!({
        "transactionData": preamble_data_b64(&preamble),
        "minResourceFee": "100",
    })));
    setup_mock_network(dir.path(), &url);

    restore_cmd(dir.path(), &[])
        .assert()
        .failure()
        .stderr(predicate::str::contains("footprint is empty"));
    assert!(!seen.lock().unwrap().submitted_anything());
}

#[test]
fn storage_restore_rejects_malformed_preamble() {
    let dir = tempdir().unwrap();
    let (url, seen) = mock_restore_rpc(Some(serde_json::json!({
        "transactionData": "bm90LXhkcg==",
        "minResourceFee": "100",
    })));
    setup_mock_network(dir.path(), &url);

    restore_cmd(dir.path(), &[])
        .assert()
        .failure()
        .stderr(predicate::str::contains("malformed"));
    assert!(!seen.lock().unwrap().submitted_anything());
}

// ---------- Offline validation ----------

#[test]
fn storage_restore_help_shows_flags() {
    sdkt()
        .args(["storage", "restore", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--contract"))
        .stdout(predicate::str::contains("--envelope"))
        .stdout(predicate::str::contains("--dry-run"))
        .stdout(predicate::str::contains("--identity"));
}

#[test]
fn storage_restore_rejects_invalid_contract_offline() {
    let envelope = invocation_envelope();
    sdkt()
        .args([
            "storage",
            "restore",
            "--contract",
            "not-a-contract",
            "--envelope",
            envelope.as_str(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid --contract"));
}

#[test]
fn storage_restore_rejects_invalid_envelope_offline() {
    sdkt()
        .args([
            "storage",
            "restore",
            "--contract",
            VALID_CONTRACT,
            "--envelope",
            "not-an-envelope",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--envelope is not a base64 XDR"));
}
