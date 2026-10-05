//! Deployment verification tests — hermetic.
//!
//! `sdkt deployment-verify` is proven against a local mock RPC serving
//! protocol-valid `LedgerEntry` XDR (the same harness pattern as the existing
//! verify/inspect suites), covering every verdict branch:
//!
//! - `MATCH`     — the local WASM's hash equals the deployed Wasm executable
//! - `DRIFT`     — a different hash is deployed
//! - `NOT_FOUND` — no contract instance entry exists at the target
//! - `UNKNOWN`   — `stellar_asset` executable (no WASM artifact exists), and
//!   no local `--wasm` supplied (nothing to compare)
//! - invalid `--contract` → exit 1 with no fabricated report
//! - exit codes: MATCH → 0, every other verdict → 1 (with a valid report)
//! - `--help` works and lists the read-only boundary

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use stellar_xdr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ContractId, ExtensionPoint,
    Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limited, Limits, ScAddress,
    ScContractInstance, ScVal, WriteXdr,
};

const CONTRACT_HEX: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// SHA-256 of the shipped `us_new.wasm` fixture (the on-chain hash the mock
/// serves for the MATCH case; taken from the fixture, never invented).
const US_NEW_HASH: &str = "551c5a9c7fd4c4a71e57e9c8a0ece1e7bd506ea065993c63db2dc35516406775";
/// A different 32-byte hash for the DRIFT case.
const OTHER_HASH: &str = "60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02";

fn us_new_wasm() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/us_new.wasm"
    ))
    .expect("fixture WASM present")
}

fn contract_address(hex_id: &str) -> ScAddress {
    let mut id = [0u8; 32];
    hex::decode_to_slice(hex_id, &mut id).unwrap();
    ScAddress::Contract(ContractId(Hash(id)))
}

fn contract_id() -> String {
    contract_address(CONTRACT_HEX).to_string()
}

fn encode(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    let mut l = Limited::new(&mut cursor, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

fn instance_entry(executable: ContractExecutable) -> String {
    encode(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: contract_address(CONTRACT_HEX),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable,
                storage: None,
            }),
        }),
        ext: LedgerEntryExt::V0,
    })
}

fn wasm_instance(hash_hex: &str) -> String {
    let mut hash = [0u8; 32];
    hex::decode_to_slice(hash_hex, &mut hash).unwrap();
    instance_entry(ContractExecutable::Wasm(Hash(hash)))
}

fn stellar_asset_instance() -> String {
    instance_entry(ContractExecutable::StellarAsset)
}

/// Mock RPC: answers the instance key with `instance_xdr` (or with no entries
/// when `instance_xdr` is `None`), plus ledger/network probes.
fn mock_rpc(instance_xdr: Option<String>) -> String {
    let instance_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(CONTRACT_HEX.to_string())).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&data);
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text[..end]
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if data.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&data).into_owned();
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or("");
            let request_json: Value = serde_json::from_str(body).unwrap_or_default();
            let method = request_json["method"].as_str().unwrap_or("");
            let response = match method {
                "getLatestLedger" => r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":28,"sequence":10000}}"#.to_string(),
                "getNetwork" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"passphrase":"Test SDF Network ; September 2015","protocolVersion":28}}"#.to_string()
                }
                "getLedgerEntries" => {
                    let requested: Vec<String> = request_json["params"]["keys"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect();
                    let entries: Vec<Value> = requested
                        .iter()
                        .filter_map(|key| {
                            if key == &instance_key {
                                instance_xdr.clone().map(|x| {
                                    serde_json::json!({
                                        "key": key,
                                        "xdr": x,
                                        "lastModifiedLedgerSeq": 1,
                                        "liveUntilLedgerSeq": 12345
                                    })
                                })
                            } else {
                                None
                            }
                        })
                        .collect();
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": 1,
                        "result": { "entries": entries, "latestLedger": 10000 }
                    })
                    .to_string()
                }
                _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                    .to_string(),
            };
            let http_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            let _ = sock.write_all(http_response.as_bytes());
        }
    });

    url
}

fn verify_json(url: &str, wasm: Option<&str>) -> Value {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.args([
        "deployment-verify",
        "--contract",
        &contract_id(),
        "--format",
        "json",
    ])
    .arg("--rpc-url")
    .arg(url);
    if let Some(w) = wasm {
        cmd.arg("--wasm").arg(w);
    }
    let out = cmd.output().expect("run sdkt");
    String::from_utf8_lossy(&out.stdout)
        .parse::<Value>()
        .expect("valid JSON report")
}

fn us_new_path() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_new.wasm").to_string()
}

#[test]
fn equal_hash_is_match_and_exits_zero() {
    let url = mock_rpc(Some(wasm_instance(US_NEW_HASH)));
    let assert = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "deployment-verify",
            "--contract",
            &contract_id(),
            "--wasm",
            &us_new_path(),
            "--format",
            "json",
            "--rpc-url",
            &url,
        ])
        .assert()
        .success();
    let report: Value = String::from_utf8_lossy(&assert.get_output().stdout)
        .parse()
        .expect("json report");
    assert_eq!(report["verdict"], "MATCH");
    assert_eq!(report["verified"], true);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["deployed_executable"], "wasm");
    assert_eq!(report["deployed_wasm_hash"], US_NEW_HASH);
    assert_eq!(report["local_wasm_hash"], US_NEW_HASH);
    assert!(report.get("reason").is_none(), "MATCH needs no reason");
}

#[test]
fn different_hash_is_drift_with_valid_report_and_exit_one() {
    let url = mock_rpc(Some(wasm_instance(OTHER_HASH)));
    let assert = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "deployment-verify",
            "--contract",
            &contract_id(),
            "--wasm",
            &us_new_path(),
            "--format",
            "json",
            "--rpc-url",
            &url,
        ])
        .assert()
        .failure()
        .code(1);
    let report: Value = String::from_utf8_lossy(&assert.get_output().stdout)
        .parse()
        .expect("drift still produces a valid report");
    assert_eq!(report["verdict"], "DRIFT");
    assert_eq!(report["verified"], false);
    assert_eq!(report["deployed_wasm_hash"], OTHER_HASH);
    assert_eq!(report["local_wasm_hash"], US_NEW_HASH);
    assert!(report["reason"]
        .as_str()
        .is_some_and(|r| r.contains(OTHER_HASH) && r.contains(US_NEW_HASH)));
}

#[test]
fn missing_instance_is_not_found() {
    let url = mock_rpc(None);
    let report = verify_json(&url, Some(&us_new_path()));
    assert_eq!(report["verdict"], "NOT_FOUND");
    assert_eq!(report["verified"], false);
    assert_eq!(report["deployed_executable"], "none");
    assert!(report["reason"]
        .as_str()
        .is_some_and(|r| r.contains("no contract instance")));
    assert!(report.get("deployed_wasm_hash").is_none());
    Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "deployment-verify",
            "--contract",
            &contract_id(),
            "--wasm",
            &us_new_path(),
            "--rpc-url",
            &url,
        ])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn stellar_asset_is_unknown_never_a_guessed_match() {
    let url = mock_rpc(Some(stellar_asset_instance()));
    let report = verify_json(&url, Some(&us_new_path()));
    assert_eq!(report["verdict"], "UNKNOWN");
    assert_eq!(report["deployed_executable"], "stellar_asset");
    assert_eq!(report["verified"], false);
    assert!(report.get("deployed_wasm_hash").is_none(), "no hash exists");
    assert!(report["reason"]
        .as_str()
        .is_some_and(|r| r.contains("stellar_asset")));
}

#[test]
fn no_local_wasm_is_unknown_even_when_deployed_hash_is_readable() {
    let url = mock_rpc(Some(wasm_instance(US_NEW_HASH)));
    let report = verify_json(&url, None);
    assert_eq!(report["verdict"], "UNKNOWN");
    // The deployed side is still reported — only the comparison is unknown.
    assert_eq!(report["deployed_wasm_hash"], US_NEW_HASH);
    assert!(report.get("local_wasm_hash").is_none());
    assert!(report["reason"]
        .as_str()
        .is_some_and(|r| r.contains("no local WASM")));
}

#[test]
fn unreachable_endpoint_exits_one_without_a_report() {
    // Port 1: guaranteed connection failure, no mock server.
    let assert = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "deployment-verify",
            "--contract",
            &contract_id(),
            "--wasm",
            &us_new_path(),
            "--format",
            "json",
            "--rpc-url",
            "http://127.0.0.1:1",
        ])
        .assert()
        .failure()
        .code(1);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        !stdout.contains("\"verdict\""),
        "a dead endpoint must not print a verdict: {stdout}"
    );
}

#[test]
fn invalid_contract_id_exits_one_without_a_report() {
    let assert = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "deployment-verify",
            "--contract",
            "NOTACONTRACT",
            "--wasm",
            &us_new_path(),
            "--format",
            "json",
            "--rpc-url",
            "http://127.0.0.1:1",
        ])
        .assert()
        .failure()
        .code(1);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(!stdout.contains("\"verdict\""), "no report: {stdout}");
}

#[test]
fn missing_wasm_file_is_an_input_error_before_the_network() {
    // A bad local path fails with the file error (exit 1), proving the
    // offline-first order — it must never reach an RPC call at all.
    Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "deployment-verify",
            "--contract",
            &contract_id(),
            "--wasm",
            "/nonexistent/missing.wasm",
            "--rpc-url",
            "http://127.0.0.1:1",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("reading WASM file"));
}

#[test]
fn help_documents_the_read_only_boundary() {
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["deployment-verify", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--contract"))
        .stdout(predicates::str::contains("--wasm"))
        .stdout(predicates::str::contains(
            "Never signs, submits, or deploys",
        ));
}

#[test]
fn us_new_wasm_fixture_hash_matches_the_constant_under_test() {
    // The MATCH/DRIFT tests compare against a constant; pin it to the real
    // fixture so a fixture change cannot silently make them vacuous.
    let meta = sdkt_wasm::parse_metadata(&us_new_wasm()).unwrap();
    assert_eq!(meta.hash, US_NEW_HASH);
}
