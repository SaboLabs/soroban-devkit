//! `sdkt token info` tests — hermetic.
//!
//! Proven against a local mock RPC serving protocol-valid `LedgerEntry`
//! XDR (the same harness pattern as `deployment_verify_test.rs`):
//!
//! - a full SEP-41 token: `is_token: true`, all 11 methods present, the
//!   four metadata values simulated and reported
//! - a non-token contract (the `us_new.wasm` counter fixture): `is_token:
//!   false`, exit 0, no simulation attempted
//! - a signature mismatch (`decimals` with the wrong signature):
//!   `signature_mismatch`, `is_token: false`
//! - undecodable spec bytes: exit 2 naming the contract
//! - a contract with no instance entry: exit 2
//! - an unreachable endpoint: exit 2, no partial report
//! - `--help` documents the command
//!
//! The mock answers, in order: `getLedgerEntries` for the instance key,
//! `getLedgerEntries` for the code key, then one `simulateTransaction`
//! per present metadata method (`name`, `symbol`, `decimals`,
//! `total_supply`). Any request outside that script — including a
//! simulation the handler must never make (e.g. `balance`) — gets a
//! JSON-RPC error, so a forbidden simulation fails the test instead of
//! silently succeeding.

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_xdr::{encode_ledger_key, scval_to_base64, LedgerKeyParams};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use stellar_xdr::{
    BytesM, ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ContractId, ExtensionPoint, Hash, Int128Parts, LedgerEntry,
    LedgerEntryData, LedgerEntryExt, Limited, Limits, ScAddress, ScContractInstance, ScSpecEntry,
    ScSpecFunctionInputV0, ScSpecFunctionV0, ScSpecTypeDef, ScString, ScSymbol, ScVal, StringM,
    WriteXdr,
};

const CONTRACT_HEX: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The WASM hash the mock instance entry points at.
const WASM_HASH_HEX: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

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

fn wasm_instance() -> String {
    let mut hash = [0u8; 32];
    hex::decode_to_slice(WASM_HASH_HEX, &mut hash).unwrap();
    instance_entry(ContractExecutable::Wasm(Hash(hash)))
}

fn code_entry(code: &[u8]) -> String {
    let mut hash = [0u8; 32];
    hex::decode_to_slice(WASM_HASH_HEX, &mut hash).unwrap();
    encode(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(hash),
            code: BytesM::try_from(code.to_vec()).unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    })
}

/// Build a minimal WASM whose only content is a `contractspecv0` custom
/// section holding the given spec entries (mirror of the storage-diff
/// fixture builder, with a real multi-byte LEB128 section size).
fn spec_wasm(entries: &[ScSpecEntry]) -> Vec<u8> {
    fn push_leb128(out: &mut Vec<u8>, mut value: usize) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    let mut section = Vec::new();
    section.push(b"contractspecv0".len() as u8);
    section.extend_from_slice(b"contractspecv0");
    for entry in entries {
        section.extend_from_slice(&entry.to_xdr(Limits::none()).unwrap());
    }

    let mut wasm = vec![0, 97, 115, 109, 1, 0, 0, 0, 0]; // \0asm + version 1
    push_leb128(&mut wasm, section.len());
    wasm.extend_from_slice(&section);
    wasm
}

fn func_entry(name: &str, inputs: Vec<(&str, ScSpecTypeDef)>) -> ScSpecEntry {
    ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
        doc: "".try_into().unwrap(),
        name: ScSymbol(name.to_string().try_into().unwrap()),
        inputs: inputs
            .into_iter()
            .map(|(n, t)| ScSpecFunctionInputV0 {
                doc: "".try_into().unwrap(),
                name: n.try_into().unwrap(),
                type_: t,
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap(),
        outputs: vec![].try_into().unwrap(),
    })
}

/// The full SEP-41 table from the issue, matched by name + signature.
fn sep41_entries() -> Vec<ScSpecEntry> {
    use ScSpecTypeDef::*;
    vec![
        func_entry("name", vec![]),
        func_entry("symbol", vec![]),
        func_entry("decimals", vec![]),
        func_entry("total_supply", vec![]),
        func_entry("balance", vec![("id", Address)]),
        func_entry("allowance", vec![("from", Address), ("spender", Address)]),
        func_entry(
            "transfer",
            vec![("from", Address), ("to", Address), ("amount", I128)],
        ),
        func_entry(
            "transfer_from",
            vec![
                ("spender", Address),
                ("from", Address),
                ("to", Address),
                ("amount", I128),
            ],
        ),
        func_entry(
            "approve",
            vec![
                ("from", Address),
                ("spender", Address),
                ("amount", I128),
                ("expiration_ledger", U32),
            ],
        ),
        func_entry("burn", vec![("from", Address), ("amount", I128)]),
        func_entry("mint", vec![("to", Address), ("amount", I128)]),
    ]
}

fn us_new_wasm() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/us_new.wasm"
    ))
    .expect("fixture WASM present")
}

/// Scripted mock RPC. `sim_values` are base64 ScVals served to
/// `simulateTransaction` in call order (name, symbol, decimals,
/// total_supply). Any unexpected request gets a JSON-RPC error.
fn mock_rpc(code: Option<Vec<u8>>, sim_values: Vec<String>) -> String {
    let instance_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(CONTRACT_HEX.to_string())).unwrap();
    let code_key =
        encode_ledger_key(&LedgerKeyParams::ContractCode(WASM_HASH_HEX.to_string())).unwrap();
    let instance_xdr = Some(wasm_instance());
    let code_xdr = code.map(|c| code_entry(&c));
    let sim_queue = Arc::new(Mutex::new(sim_values));

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
                            } else if key == &code_key {
                                code_xdr.clone().map(|x| {
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
                "simulateTransaction" => {
                    let next = sim_queue.lock().unwrap().first().cloned();
                    match next {
                        Some(xdr) => {
                            sim_queue.lock().unwrap().remove(0);
                            serde_json::json!({
                                "jsonrpc": "2.0", "id": 1,
                                "result": {
                                    "results": [{"xdr": xdr, "auth": []}],
                                    "latestLedger": 10000,
                                    "events": []
                                }
                            })
                            .to_string()
                        }
                        None => {
                            // A simulation the handler must never make.
                            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"unexpected simulateTransaction call"}}"#.to_string()
                        }
                    }
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

/// Mock RPC that serves no instance entry at all (nonexistent contract).
fn mock_rpc_no_instance() -> String {
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
            let response = serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "result": { "entries": [], "latestLedger": 10000 }
            })
            .to_string();
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

fn token_sim_values() -> Vec<String> {
    vec![
        scval_to_base64(&ScVal::String(ScString(
            StringM::try_from(b"Test Token".to_vec()).unwrap(),
        )))
        .unwrap(),
        scval_to_base64(&ScVal::String(ScString(
            StringM::try_from(b"TST".to_vec()).unwrap(),
        )))
        .unwrap(),
        scval_to_base64(&ScVal::U32(7)).unwrap(),
        scval_to_base64(&ScVal::I128(Int128Parts {
            hi: 0,
            lo: 1_000_000,
        }))
        .unwrap(),
    ]
}

fn token_info_json(url: &str) -> (Value, String) {
    let out = Command::cargo_bin("sdkt")
        .unwrap()
        .args(["token", "info", &contract_id(), "--format", "json"])
        .arg("--rpc-url")
        .arg(url)
        .output()
        .expect("run sdkt");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "token info must exit 0, stderr: {stderr}"
    );
    (stdout.parse::<Value>().expect("valid JSON report"), stderr)
}

fn method_presence(report: &Value, name: &str) -> String {
    report["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("method {name} missing from report"))["presence"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn full_sep41_token_is_token_with_simulated_metadata() {
    let url = mock_rpc(Some(spec_wasm(&sep41_entries())), token_sim_values());
    let (report, _) = token_info_json(&url);
    assert_eq!(report["is_token"], true);
    assert_eq!(report["contract"], contract_id());
    assert_eq!(report["methods"].as_array().unwrap().len(), 11);
    for name in [
        "name",
        "symbol",
        "decimals",
        "total_supply",
        "balance",
        "allowance",
        "transfer",
        "transfer_from",
        "approve",
        "burn",
        "mint",
    ] {
        assert_eq!(method_presence(&report, name), "present", "{name}");
    }
    assert_eq!(report["metadata"]["name"]["value"], "Test Token");
    assert_eq!(report["metadata"]["symbol"]["value"], "TST");
    assert_eq!(report["metadata"]["decimals"]["value"], 7);
    assert_eq!(report["metadata"]["total_supply"]["value"], "1000000");
    assert!(report["metadata"]["name"].get("error").is_none());
}

#[test]
fn full_sep41_token_pretty_output_reports_values() {
    let url = mock_rpc(Some(spec_wasm(&sep41_entries())), token_sim_values());
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["token", "info", &contract_id()])
        .arg("--rpc-url")
        .arg(&url)
        .assert()
        .success()
        .stdout(predicates::str::contains("is_token: true"))
        .stdout(predicates::str::contains("\"Test Token\""))
        .stdout(predicates::str::contains("decimals"))
        .stdout(predicates::str::contains("1000000"));
}

#[test]
fn non_token_contract_is_not_a_token_and_exits_zero() {
    // The counter fixture has no SEP-41 method at all, so no simulation
    // is attempted — the mock's empty sim queue proves it (any
    // simulateTransaction call would get a JSON-RPC error and fail).
    let url = mock_rpc(Some(us_new_wasm()), vec![]);
    let (report, _) = token_info_json(&url);
    assert_eq!(report["is_token"], false);
    for name in ["name", "symbol", "decimals", "total_supply"] {
        assert_eq!(method_presence(&report, name), "absent", "{name}");
        assert_eq!(report["metadata"][name]["value"], Value::Null);
        assert!(
            report["metadata"][name]["error"]
                .as_str()
                .is_some_and(|e| e.contains("absent")),
            "metadata {name} must explain why it is unavailable"
        );
    }
}

#[test]
fn signature_mismatch_is_not_present_and_not_a_token() {
    // `decimals` with an address parameter: right name, wrong signature.
    let mut entries = sep41_entries();
    entries[2] = func_entry("decimals", vec![("id", ScSpecTypeDef::Address)]);
    // Only the three other metadata methods are simulated (decimals is
    // skipped): name, symbol, total_supply.
    let mut sims = token_sim_values();
    sims.remove(2); // drop the decimals value
    let url = mock_rpc(Some(spec_wasm(&entries)), sims);
    let (report, _) = token_info_json(&url);
    assert_eq!(report["is_token"], false);
    assert_eq!(method_presence(&report, "decimals"), "signature_mismatch");
    assert_eq!(method_presence(&report, "name"), "present");
    assert_eq!(report["metadata"]["decimals"]["value"], Value::Null);
    assert!(report["metadata"]["decimals"]["error"]
        .as_str()
        .is_some_and(|e| e.contains("signature")));
}

#[test]
fn undecodable_spec_exits_two_naming_the_contract() {
    let url = mock_rpc(Some(b"this is not a wasm".to_vec()), vec![]);
    let out = Command::cargo_bin("sdkt")
        .unwrap()
        .args(["token", "info", &contract_id()])
        .arg("--rpc-url")
        .arg(&url)
        .output()
        .expect("run sdkt");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&contract_id()),
        "exit-2 error must name the contract: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("is_token"),
        "an undecodable contract is never reported as not-a-token: {stdout}"
    );
}

#[test]
fn nonexistent_contract_exits_two() {
    let url = mock_rpc_no_instance();
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["token", "info", &contract_id()])
        .arg("--rpc-url")
        .arg(&url)
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains(contract_id()));
}

#[test]
fn unreachable_endpoint_exits_two() {
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["token", "info", &contract_id()])
        .arg("--rpc-url")
        .arg("http://127.0.0.1:1")
        .assert()
        .failure()
        .code(2);
}

#[test]
fn help_documents_token_info() {
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["token", "info", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("CONTRACT_ID"))
        .stdout(predicates::str::contains("--format"));
}
