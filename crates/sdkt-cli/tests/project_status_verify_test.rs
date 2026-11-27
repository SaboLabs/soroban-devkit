//! Integration tests for `sdkt project status --verify` against a stubbed RPC server (#542).

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_core::deployment::{DeploymentRecord, DeploymentRecordFile, DEPLOYMENT_RECORD_FILE};
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use stellar_xdr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ContractId, ExtensionPoint,
    Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt, LedgerKey, Limited, Limits, ReadXdr,
    ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use tempfile::TempDir;

const HASH_ALPHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_BETA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_DRIFT: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

const CONTRACT_A_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const CONTRACT_B_HEX: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn contract_id_strkey(hex_id: &str) -> String {
    let mut id = [0u8; 32];
    hex::decode_to_slice(hex_id, &mut id).unwrap();
    format!("{}", stellar_strkey::Contract(id))
}

fn contract_address(hex_id: &str) -> ScAddress {
    let mut id = [0u8; 32];
    hex::decode_to_slice(hex_id, &mut id).unwrap();
    ScAddress::Contract(ContractId(Hash(id)))
}

fn encode_entry(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    let mut l = Limited::new(&mut cursor, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

fn wasm_instance_xdr(hex_contract_id: &str, hash_hex: &str) -> String {
    let mut hash = [0u8; 32];
    hex::decode_to_slice(hash_hex, &mut hash).unwrap();
    encode_entry(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: contract_address(hex_contract_id),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(hash)),
                storage: None,
            }),
        }),
        ext: LedgerEntryExt::V0,
    })
}

#[derive(Clone)]
enum MockOutcome {
    MatchWasm(String),
    NotFound,
    RpcError(String),
}

fn read_http_request(sock: &mut TcpStream) -> String {
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
    String::from_utf8_lossy(&data).into_owned()
}

fn spawn_mock_rpc(outcomes: HashMap<String, MockOutcome>) -> String {
    spawn_mock_rpc_with_recorder(outcomes, None)
}

fn spawn_mock_rpc_with_recorder(
    outcomes: HashMap<String, MockOutcome>,
    recorder: Option<Arc<std::sync::Mutex<Vec<String>>>>,
) -> String {
    let mut key_map: HashMap<String, (String, MockOutcome)> = HashMap::new();
    for (contract_hex, outcome) in outcomes {
        let key = encode_ledger_key(&LedgerKeyParams::ContractData(contract_hex.clone())).unwrap();
        key_map.insert(key, (contract_hex, outcome));
    }
    let key_map = Arc::new(key_map);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let request = read_http_request(&mut sock);
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or("");
            let request_json: Value = serde_json::from_str(body).unwrap_or_default();
            let method = request_json["method"].as_str().unwrap_or("");

            let response = match method {
                "getLatestLedger" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":28,"sequence":10000}}"#.to_string()
                }
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

                    if let Some(rec) = &recorder {
                        rec.lock().unwrap().extend(requested.clone());
                    }

                    let mut entries = Vec::new();
                    let mut error_response = None;

                    for key in &requested {
                        if let Some((contract_hex, outcome)) = key_map.get(key) {
                            match outcome {
                                MockOutcome::MatchWasm(hash) => {
                                    let xdr = wasm_instance_xdr(contract_hex, hash);
                                    entries.push(serde_json::json!({
                                        "key": key,
                                        "xdr": xdr,
                                        "lastModifiedLedgerSeq": 1,
                                    }));
                                }
                                MockOutcome::NotFound => {}
                                MockOutcome::RpcError(msg) => {
                                    error_response = Some(format!(
                                        r#"{{"jsonrpc":"2.0","id":1,"error":{{"code":-32603,"message":"{msg}"}}}}"#
                                    ));
                                    break;
                                }
                            }
                        }
                    }

                    if let Some(err) = error_response {
                        err
                    } else {
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "result": {
                                "entries": entries,
                                "latestLedger": 10000
                            }
                        })
                        .to_string()
                    }
                }
                _ => {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string()
                }
            };

            let http_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            let _ = sock.write_all(http_response.as_bytes());
            let _ = sock.flush();
        }
    });

    url
}

fn sdkt(project: &Path, network_store: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(project)
        .env("SDKT_NETWORK_DIR", network_store)
        .env("SDKT_IDENTITY_DIR", project.join("identity"));
    cmd
}

fn record(contract_id: &str, wasm_hash: &str, network: &str) -> DeploymentRecord {
    DeploymentRecord {
        contract_id: contract_id.to_string(),
        wasm_hash: wasm_hash.to_string(),
        network: network.to_string(),
        timestamp: 1_700_000_000,
        salt: None,
    }
}

fn write_records(project: &Path, entries: &[(&str, &str, &str, &str)]) {
    let mut file = DeploymentRecordFile::default();
    for (scope, alias, contract_id, wasm_hash) in entries {
        file.set_record(scope, alias, record(contract_id, wasm_hash, scope));
    }
    file.write(project.join(DEPLOYMENT_RECORD_FILE))
        .expect("write deployment records");
}

#[test]
fn status_verify_all_match_exits_0() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    let cid_a = contract_id_strkey(CONTRACT_A_HEX);
    let cid_b = contract_id_strkey(CONTRACT_B_HEX);

    write_records(
        project.path(),
        &[
            ("testnet", "alpha", &cid_a, HASH_ALPHA),
            ("testnet", "beta", &cid_b, HASH_BETA),
        ],
    );

    let mut outcomes = HashMap::new();
    outcomes.insert(
        CONTRACT_A_HEX.to_string(),
        MockOutcome::MatchWasm(HASH_ALPHA.to_string()),
    );
    outcomes.insert(
        CONTRACT_B_HEX.to_string(),
        MockOutcome::MatchWasm(HASH_BETA.to_string()),
    );
    let rpc_url = spawn_mock_rpc(outcomes);

    sdkt(project.path(), network_store.path())
        .args(["project", "status", "--verify", "--rpc-url", &rpc_url])
        .assert()
        .success()
        .stdout(predicates::str::contains("alpha: MATCH"))
        .stdout(predicates::str::contains("beta: MATCH"));
}

#[test]
fn status_verify_drift_shows_both_hashes_and_exits_1() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    let cid_a = contract_id_strkey(CONTRACT_A_HEX);
    let cid_b = contract_id_strkey(CONTRACT_B_HEX);

    write_records(
        project.path(),
        &[
            ("testnet", "alpha", &cid_a, HASH_ALPHA),
            ("testnet", "beta", &cid_b, HASH_BETA),
        ],
    );

    let mut outcomes = HashMap::new();
    outcomes.insert(
        CONTRACT_A_HEX.to_string(),
        MockOutcome::MatchWasm(HASH_ALPHA.to_string()),
    );
    outcomes.insert(
        CONTRACT_B_HEX.to_string(),
        MockOutcome::MatchWasm(HASH_DRIFT.to_string()),
    );
    let rpc_url = spawn_mock_rpc(outcomes);

    sdkt(project.path(), network_store.path())
        .args(["project", "status", "--verify", "--rpc-url", &rpc_url])
        .assert()
        .code(1)
        .stdout(predicates::str::contains("alpha: MATCH"))
        .stdout(predicates::str::contains(format!(
            "beta: DRIFT (recorded: {HASH_BETA}, on-chain: {HASH_DRIFT})"
        )));
}

#[test]
fn status_verify_missing_contract_exits_1() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    let cid_a = contract_id_strkey(CONTRACT_A_HEX);
    let cid_b = contract_id_strkey(CONTRACT_B_HEX);

    write_records(
        project.path(),
        &[
            ("testnet", "alpha", &cid_a, HASH_ALPHA),
            ("testnet", "beta", &cid_b, HASH_BETA),
        ],
    );

    let mut outcomes = HashMap::new();
    outcomes.insert(
        CONTRACT_A_HEX.to_string(),
        MockOutcome::MatchWasm(HASH_ALPHA.to_string()),
    );
    outcomes.insert(CONTRACT_B_HEX.to_string(), MockOutcome::NotFound);
    let rpc_url = spawn_mock_rpc(outcomes);

    sdkt(project.path(), network_store.path())
        .args(["project", "status", "--verify", "--rpc-url", &rpc_url])
        .assert()
        .code(1)
        .stdout(predicates::str::contains("alpha: MATCH"))
        .stdout(predicates::str::contains("beta: MISSING"));
}

#[test]
fn status_verify_rpc_error_per_record_continues_and_exits_1() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    let cid_a = contract_id_strkey(CONTRACT_A_HEX);
    let cid_b = contract_id_strkey(CONTRACT_B_HEX);

    write_records(
        project.path(),
        &[
            ("testnet", "alpha", &cid_a, HASH_ALPHA),
            ("testnet", "beta", &cid_b, HASH_BETA),
        ],
    );

    let mut outcomes = HashMap::new();
    outcomes.insert(
        CONTRACT_A_HEX.to_string(),
        MockOutcome::RpcError("simulated rpc failure".to_string()),
    );
    outcomes.insert(
        CONTRACT_B_HEX.to_string(),
        MockOutcome::MatchWasm(HASH_BETA.to_string()),
    );
    let rpc_url = spawn_mock_rpc(outcomes);

    sdkt(project.path(), network_store.path())
        .args(["project", "status", "--verify", "--rpc-url", &rpc_url])
        .assert()
        .code(1)
        .stdout(predicates::str::contains("alpha: ERROR"))
        .stdout(predicates::str::contains("beta: MATCH"));
}

#[test]
fn status_verify_json_shape_and_summary() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    let cid_a = contract_id_strkey(CONTRACT_A_HEX);
    let cid_b = contract_id_strkey(CONTRACT_B_HEX);

    write_records(
        project.path(),
        &[
            ("testnet", "alpha", &cid_a, HASH_ALPHA),
            ("testnet", "beta", &cid_b, HASH_BETA),
        ],
    );

    let mut outcomes = HashMap::new();
    outcomes.insert(
        CONTRACT_A_HEX.to_string(),
        MockOutcome::MatchWasm(HASH_ALPHA.to_string()),
    );
    outcomes.insert(
        CONTRACT_B_HEX.to_string(),
        MockOutcome::MatchWasm(HASH_DRIFT.to_string()),
    );
    let rpc_url = spawn_mock_rpc(outcomes);

    let output = sdkt(project.path(), network_store.path())
        .args([
            "project",
            "status",
            "--verify",
            "--format",
            "json",
            "--rpc-url",
            &rpc_url,
        ])
        .output()
        .expect("status verify json runs");

    assert_eq!(output.status.code(), Some(1));

    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["status"], "deployed");
    assert_eq!(parsed["network"], "testnet");

    let summary = &parsed["summary"];
    assert_eq!(summary["match"], 1);
    assert_eq!(summary["drift"], 1);
    assert_eq!(summary["missing"], 0);
    assert_eq!(summary["error"], 0);

    let deployments = parsed["deployments"].as_array().unwrap();
    assert_eq!(deployments.len(), 2);

    assert_eq!(deployments[0]["alias"], "alpha");
    assert_eq!(deployments[0]["contract_id"], cid_a);
    assert_eq!(deployments[0]["wasm_hash"], HASH_ALPHA);
    assert_eq!(deployments[0]["status"], "MATCH");
    assert_eq!(deployments[0]["on_chain_wasm_hash"], HASH_ALPHA);

    assert_eq!(deployments[1]["alias"], "beta");
    assert_eq!(deployments[1]["contract_id"], cid_b);
    assert_eq!(deployments[1]["wasm_hash"], HASH_BETA);
    assert_eq!(deployments[1]["status"], "DRIFT");
    assert_eq!(deployments[1]["on_chain_wasm_hash"], HASH_DRIFT);
}

#[test]
fn status_verify_empty_deployments_exits_0() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    // Pretty mode
    sdkt(project.path(), network_store.path())
        .args(["project", "status", "--verify"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No deployments recorded"));

    // JSON mode
    let output = sdkt(project.path(), network_store.path())
        .args(["project", "status", "--verify", "--format", "json"])
        .output()
        .expect("status verify empty json runs");
    assert!(output.status.success());

    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["status"], "no_deployments");
    assert_eq!(parsed["network"], "testnet");
    assert_eq!(parsed["deployments"], serde_json::json!([]));
    assert_eq!(parsed["summary"]["match"], 0);
    assert_eq!(parsed["summary"]["drift"], 0);
    assert_eq!(parsed["summary"]["missing"], 0);
    assert_eq!(parsed["summary"]["error"], 0);
}

#[test]
fn status_without_verify_remains_offline_and_unchanged() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    let cid_a = contract_id_strkey(CONTRACT_A_HEX);
    write_records(project.path(), &[("testnet", "alpha", &cid_a, HASH_ALPHA)]);

    // Using an unreachable port without --verify succeeds without attempting network contact.
    sdkt(project.path(), network_store.path())
        .args(["project", "status", "--rpc-url", "http://127.0.0.1:1"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Project deployment status (network: testnet):",
        ))
        .stdout(predicates::str::contains("alpha"))
        .stdout(predicates::str::contains(format!("Contract ID: {cid_a}")))
        .stdout(predicates::str::contains(format!(
            "WASM hash:   {HASH_ALPHA}"
        )));
}

#[test]
fn status_verify_never_requests_contract_code_entry() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    let cid_a = contract_id_strkey(CONTRACT_A_HEX);
    write_records(project.path(), &[("testnet", "alpha", &cid_a, HASH_ALPHA)]);

    let recorded_keys = Arc::new(std::sync::Mutex::new(Vec::new()));

    let mut outcomes = HashMap::new();
    outcomes.insert(
        CONTRACT_A_HEX.to_string(),
        MockOutcome::MatchWasm(HASH_ALPHA.to_string()),
    );
    let rpc_url = spawn_mock_rpc_with_recorder(outcomes, Some(Arc::clone(&recorded_keys)));

    sdkt(project.path(), network_store.path())
        .args(["project", "status", "--verify", "--rpc-url", &rpc_url])
        .assert()
        .success()
        .stdout(predicates::str::contains("alpha: MATCH"));

    let keys = recorded_keys.lock().unwrap().clone();
    assert!(
        !keys.is_empty(),
        "expected at least one ledger key requested"
    );

    let expected_instance_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(CONTRACT_A_HEX.to_string())).unwrap();
    let expected_code_key =
        encode_ledger_key(&LedgerKeyParams::ContractCode(HASH_ALPHA.to_string())).unwrap();

    // Assert that the instance singleton key was requested
    assert!(
        keys.contains(&expected_instance_key),
        "expected instance singleton key to be requested"
    );

    // Assert that the contract code key was never requested (no bytecode download)
    assert!(
        !keys.contains(&expected_code_key),
        "expected contract code key NOT to be requested (no bytecode download)"
    );

    // Assert that no requested key is a ContractCode ledger key
    for key_b64 in &keys {
        let decoded =
            LedgerKey::from_xdr_base64(key_b64, Limits::none()).expect("valid ledger key XDR");
        assert!(
            !matches!(decoded, LedgerKey::ContractCode(_)),
            "found ContractCode ledger key request: {key_b64}"
        );
    }
}
