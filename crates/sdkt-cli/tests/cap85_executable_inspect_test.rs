//! CAP-85 (Protocol 28) executable inspection tests.
//!
//! Proves the real `sdkt inspect` flow handles every `ContractExecutable`
//! variant against protocol-valid XDR — not a mocked Rust enum:
//!
//! - `ExternalRef` (owner + tag) is resolved through the read-only owner
//!   lookup and reports the referenced Wasm hash + ABI.
//! - A missing owner entry fails with an explicit, actionable error.
//! - `StellarAsset` is never reported as a Wasm contract.
//!
//! Ledger entries are encoded with the real `stellar-xdr` 28 types and served
//! by a local mock RPC over HTTP, exactly as the existing offline contract
//! tests do.

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use stellar_xdr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ContractExecutableExternalRef, ContractId, ExtensionPoint, Hash,
    LedgerEntry, LedgerEntryData, LedgerEntryExt, Limited, Limits, ScAddress, ScContractInstance,
    ScString, ScVal, StringM, WriteXdr,
};

const CONTRACT_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OWNER_HEX: &str = "4242424242424242424242424242424242424242424242424242424242424242";
/// Hash the owner's executable-tag entry names (matches `us_new.wasm` on chain).
const CODE_HASH_HEX: &str = "551c5a9c7fd4c4a71e57e9c8a0ece1e7bd506ea065993c63db2dc35516406775";
const TAG: &str = "release-28";

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").expect("sdkt binary built")
}

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

/// The contract-under-test's `C...` id (same bytes as `CONTRACT_HEX`).
fn contract_id() -> String {
    contract_address(CONTRACT_HEX).to_string()
}

fn owner_id() -> String {
    contract_address(OWNER_HEX).to_string()
}

fn tag_string() -> ScString {
    ScString(StringM::try_from(TAG.as_bytes().to_vec()).unwrap())
}

fn encode(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    let mut l = Limited::new(&mut cursor, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

/// Instance entry whose executable is the CAP-85 external reference.
fn external_ref_instance_xdr() -> String {
    encode(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: contract_address(CONTRACT_HEX),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::ExternalRef(ContractExecutableExternalRef {
                    executable_owner: contract_address(OWNER_HEX),
                    tag: tag_string(),
                }),
                storage: None,
            }),
        }),
        ext: LedgerEntryExt::V0,
    })
}

/// Instance entry for a Stellar Asset Contract.
fn stellar_asset_instance_xdr() -> String {
    encode(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: contract_address(CONTRACT_HEX),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::StellarAsset,
                storage: None,
            }),
        }),
        ext: LedgerEntryExt::V0,
    })
}

/// The owner's persistent executable-tag entry: keyed by `SCV_EXECUTABLE_TAG`,
/// valued with the raw 32 hash bytes (the only value shape CAP-85 permits).
fn owner_tag_entry_xdr() -> String {
    encode(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: contract_address(OWNER_HEX),
            key: ScVal::ExecutableTag(tag_string()),
            durability: ContractDataDurability::Persistent,
            val: ScVal::Bytes(stellar_xdr::ScBytes(
                stellar_xdr::BytesM::try_from(hex::decode(CODE_HASH_HEX).unwrap()).unwrap(),
            )),
        }),
        ext: LedgerEntryExt::V0,
    })
}

/// The referenced Wasm code entry (what `getLedgerEntries` returns for a hash).
fn code_entry_xdr() -> String {
    let mut hash = [0u8; 32];
    hex::decode_to_slice(CODE_HASH_HEX, &mut hash).unwrap();
    encode(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(hash),
            code: stellar_xdr::BytesM::try_from(us_new_wasm()).unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    })
}

fn read_rpc_request(sock: &mut TcpStream) -> String {
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

/// Mock Soroban RPC:
/// - `serve_tag = true` answers the owner's executable-tag lookup with the
///   hash entry; `false` answers with no entries (missing entry).
/// - `instance_xdr` selects the executable variant under test.
/// - The referenced `ContractCode` entry is always served (so a resolved
///   inspection can fetch bytecode + ABI exactly like a plain Wasm contract).
fn mock_rpc(instance_xdr: String, serve_tag: bool) -> String {
    let instance_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(CONTRACT_HEX.to_string())).unwrap();
    let tag_key = encode_ledger_key(&LedgerKeyParams::ContractDataEntry {
        contract: owner_id(),
        key: ScVal::ExecutableTag(tag_string()),
        durability: ContractDataDurability::Persistent,
    })
    .unwrap();
    let code_key =
        encode_ledger_key(&LedgerKeyParams::ContractCode(CODE_HASH_HEX.to_string())).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else {
                break;
            };
            let request = read_rpc_request(&mut sock);
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or("");
            let request_json: Value = serde_json::from_str(body).unwrap_or_default();
            let method = request_json["method"].as_str().unwrap_or("");
            let response = match method {
                "getLatestLedger" => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":28,"sequence":10000}}"#
                        .to_string()
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
                            let (xdr, entry_key) = if key == &instance_key {
                                (Some(instance_xdr.clone()), instance_key.clone())
                            } else if key == &tag_key {
                                if serve_tag {
                                    (Some(owner_tag_entry_xdr()), tag_key.clone())
                                } else {
                                    (None, tag_key.clone())
                                }
                            } else if key == &code_key {
                                (Some(code_entry_xdr()), code_key.clone())
                            } else {
                                (None, key.clone())
                            };
                            xdr.map(|x| {
                                serde_json::json!({
                                    "key": entry_key,
                                    "xdr": x,
                                    "lastModifiedLedgerSeq": 1,
                                    "liveUntilLedgerSeq": 12345
                                })
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": 1,
                        "result": { "entries": entries, "latestLedger": 10000 }
                    })
                    .to_string()
                }
                _ => {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                        .to_string()
                }
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

#[test]
fn external_ref_resolves_to_wasm_hash_and_abi() {
    let url = mock_rpc(external_ref_instance_xdr(), true);
    let output = sdkt()
        .args(["inspect", &contract_id(), "--format", "json", "--rpc-url"])
        .arg(&url)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&output).expect("valid JSON");

    // The referenced hash must be reported as the contract's executable code.
    assert_eq!(v["wasm_hash"], CODE_HASH_HEX, "resolved hash: {v}");
    assert!(
        v["wasm_size"].as_u64().is_some(),
        "resolved hash must let the code entry be fetched: {v}"
    );
    let functions = v["abi"]["functions"]
        .as_array()
        .expect("ABI parsed from the resolved code");
    assert!(
        functions.iter().any(|f| f == "hello" || f == "increment"),
        "expected us_new.wasm functions, got {functions:?}"
    );
}

#[test]
fn external_ref_missing_owner_entry_fails_explicitly() {
    let url = mock_rpc(external_ref_instance_xdr(), false);
    let assert = sdkt()
        .args(["inspect", &contract_id(), "--format", "json", "--rpc-url"])
        .arg(&url)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("no executable tag entry") && stderr.contains(TAG),
        "expected an actionable missing-entry error naming the tag, got: {stderr}"
    );
    // No fabricated hash may leak into stdout.
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        !stdout.contains("wasm_hash"),
        "must not report a hash: {stdout}"
    );
}

#[test]
fn stellar_asset_is_never_reported_as_wasm() {
    let url = mock_rpc(stellar_asset_instance_xdr(), false);
    let assert = sdkt()
        .args(["inspect", &contract_id(), "--format", "json", "--rpc-url"])
        .arg(&url)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("stellar_asset") && stderr.contains("not a Wasm contract"),
        "expected an explicit non-Wasm error, got: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        !stdout.contains(CODE_HASH_HEX),
        "must not report a hash: {stdout}"
    );
}
