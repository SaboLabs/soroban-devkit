//! `sdkt diff --old-contract / --new-contract` CLI tests against a stubbed
//! JSON-RPC server (#524).
//!
//! The mock routes `getLedgerEntries` per ledger key: each contract's
//! contract-data key serves a `CONTRACT_DATA` instance entry pointing at that
//! contract's WASM hash, and each contract-code key serves a `CONTRACT_CODE`
//! entry carrying the matching fixture WASM. Any other key serves empty
//! entries, which is how the CLI learns a contract does not exist on the
//! network. No live network, no identity, no funds.
//!
//! The fetch under test is the same composition `verify --upgrade-safety`
//! uses (`inspect_contract` → `get_wasm_bytecode`); the fetched bytes then
//! flow through the existing `diff_wasm` path unchanged, so a contract-ID
//! diff must match the equivalent two-file diff apart from the recorded
//! source fields.

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;
use stellar_xdr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ContractId, ExtensionPoint, Hash, LedgerEntry, LedgerEntryData,
    LedgerEntryExt, Limited, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use tempfile::tempdir;

static OLD_WASM: &[u8] = include_bytes!("fixtures/us_old.wasm");
static NEW_WASM: &[u8] = include_bytes!("fixtures/us_new.wasm");

/// A deployed-testnet contract StrKey; its hash is `09ba7d2a...06c`.
const OLD_CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const OLD_CONTRACT_HEX: &str = "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c";
/// On-chain WASM hash the mock serves for `OLD_CONTRACT_ID`.
const OLD_WASM_HASH_HEX: &str = "60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02";

/// Second mock contract, built from a fixed 32-byte hash (never deployed;
/// the mock is the whole network here).
fn new_contract_id() -> (String, String) {
    let hash = [0x22u8; 32];
    (
        format!("{}", stellar_strkey::Contract(hash)),
        hex::encode(hash),
    )
}
/// On-chain WASM hash the mock serves for the second contract.
const NEW_WASM_HASH_HEX: &str = "3333333333333333333333333333333333333333333333333333333333333333";

/// A well-formed contract ID the mock knows nothing about (empty entries).
fn unknown_contract_id() -> String {
    format!("{}", stellar_strkey::Contract([0x44u8; 32]))
}

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
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

/// Serialize a `LedgerEntry` to its base64 XDR wire form.
fn encode_ledger_entry(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut l = Limited::new(&mut buf, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

/// Build the base64 `LedgerEntry` for a `CONTRACT_DATA` entry whose
/// `ContractInstance` executable points at `wasm_hash` — the shape
/// `inspect_contract` expects so it can resolve the on-chain WASM hash.
fn contract_data_entry_xdr(wasm_hash: [u8; 32]) -> String {
    encode_ledger_entry(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(wasm_hash)),
                storage: None,
            }),
        }),
        ext: LedgerEntryExt::V0,
    })
}

/// Build the base64 `LedgerEntry` for a `CONTRACT_CODE` entry carrying `code` —
/// the shape `get_wasm_bytecode` reads back into raw bytes.
fn contract_code_entry_xdr(wasm_hash: [u8; 32], code: &[u8]) -> String {
    encode_ledger_entry(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(wasm_hash),
            code: code.to_vec().try_into().unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    })
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

fn entries_body(key: &str, xdr: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{key}","xdr":"{xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
    )
}

/// Mock JSON-RPC server covering the whole `diff --old-contract /
/// --new-contract` flow.
///
/// `getLedgerEntries` is routed by the encoded ledger key in the request
/// body: each contract-data key serves its instance entry, each
/// contract-code key serves its code entry, and any other key serves empty
/// entries (contract not found).
fn mock_rpc_server() -> String {
    let (_new_contract_id, new_contract_hex) = new_contract_id();

    let old_data_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(OLD_CONTRACT_HEX.to_string())).unwrap();
    let old_code_key = encode_ledger_key(&LedgerKeyParams::ContractCode(
        OLD_WASM_HASH_HEX.to_string(),
    ))
    .unwrap();
    let new_data_key = encode_ledger_key(&LedgerKeyParams::ContractData(new_contract_hex)).unwrap();
    let new_code_key = encode_ledger_key(&LedgerKeyParams::ContractCode(
        NEW_WASM_HASH_HEX.to_string(),
    ))
    .unwrap();

    let mut old_wasm_hash = [0u8; 32];
    hex::decode_to_slice(OLD_WASM_HASH_HEX, &mut old_wasm_hash).unwrap();
    let mut new_wasm_hash = [0u8; 32];
    hex::decode_to_slice(NEW_WASM_HASH_HEX, &mut new_wasm_hash).unwrap();

    let old_data_xdr = contract_data_entry_xdr(old_wasm_hash);
    let old_code_xdr = contract_code_entry_xdr(old_wasm_hash, OLD_WASM);
    let new_data_xdr = contract_data_entry_xdr(new_wasm_hash);
    let new_code_xdr = contract_code_entry_xdr(new_wasm_hash, NEW_WASM);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_http_request(&mut sock);
            if req.is_empty() {
                continue;
            }
            let body = if req.contains("\"getLedgerEntries\"") {
                if req.contains(old_data_key.as_str()) {
                    entries_body(&old_data_key, &old_data_xdr)
                } else if req.contains(old_code_key.as_str()) {
                    entries_body(&old_code_key, &old_code_xdr)
                } else if req.contains(new_data_key.as_str()) {
                    entries_body(&new_data_key, &new_data_xdr)
                } else if req.contains(new_code_key.as_str()) {
                    entries_body(&new_code_key, &new_code_xdr)
                } else {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"entries":[],"latestLedger":100}}"#
                        .to_string()
                }
            } else {
                r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                    .to_string()
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });
    thread::sleep(Duration::from_millis(50));
    format!("http://127.0.0.1:{port}")
}

/// Run `sdkt diff` against the mock network and return its output.
fn diff_against_mock(dir: &std::path::Path, extra_args: &[&str]) -> std::process::Output {
    let url = mock_rpc_server();
    add_mock_profile(dir, &url);
    let mut cmd = sdkt_isolated(dir);
    cmd.arg("diff");
    cmd.args(extra_args);
    cmd.args(["--network-profile", "mocknet"]);
    cmd.output().unwrap()
}

fn diff_json(dir: &std::path::Path, extra_args: &[&str]) -> serde_json::Value {
    let output = diff_against_mock(dir, extra_args);
    assert!(
        output.status.success(),
        "diff must succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"))
}

fn two_file_diff_json(dir: &std::path::Path) -> serde_json::Value {
    let output = sdkt_isolated(dir)
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "two-file diff must succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"))
}

#[test]
fn diff_contract_sides_match_two_file_diff_apart_from_sources() {
    let dir = tempdir().unwrap();
    let (new_contract_id, _) = new_contract_id();
    let contract_json = diff_json(
        dir.path(),
        &[
            "--old-contract",
            OLD_CONTRACT_ID,
            "--new-contract",
            &new_contract_id,
            "--format",
            "json",
        ],
    );
    let file_json = two_file_diff_json(dir.path());

    // The JSON records which side came from a contract ID.
    assert_eq!(contract_json["old_source"]["kind"], "contract");
    assert_eq!(contract_json["old_source"]["contract_id"], OLD_CONTRACT_ID);
    assert_eq!(contract_json["new_source"]["kind"], "contract");
    assert_eq!(
        contract_json["new_source"]["contract_id"], new_contract_id,
        "got: {contract_json}"
    );

    // Apart from the source fields, the findings are identical.
    let mut stripped = contract_json.clone();
    stripped.as_object_mut().unwrap().remove("old_source");
    stripped.as_object_mut().unwrap().remove("new_source");
    assert_eq!(stripped, file_json, "contract diff must equal file diff");

    // And the classic two-file JSON carries no source fields at all.
    assert!(file_json.get("old_source").is_none());
    assert!(file_json.get("new_source").is_none());
}

#[test]
fn diff_mixed_file_and_contract_sides() {
    let dir = tempdir().unwrap();
    let (new_contract_id, _) = new_contract_id();
    let mixed = diff_json(
        dir.path(),
        &[
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-contract",
            &new_contract_id,
            "--format",
            "json",
        ],
    );
    let file_json = two_file_diff_json(dir.path());

    assert_eq!(mixed["old_source"]["kind"], "file");
    assert!(
        mixed["old_source"]["path"]
            .as_str()
            .unwrap()
            .ends_with("us_old.wasm"),
        "got: {mixed}"
    );
    assert_eq!(mixed["new_source"]["kind"], "contract");
    assert_eq!(mixed["new_source"]["contract_id"], new_contract_id);

    let mut stripped = mixed.clone();
    stripped.as_object_mut().unwrap().remove("old_source");
    stripped.as_object_mut().unwrap().remove("new_source");
    assert_eq!(stripped, file_json, "mixed diff must equal file diff");
}

#[test]
fn diff_contract_conflict_is_usage_error() {
    let dir = tempdir().unwrap();
    // --old-contract vs --old-wasm, other side sourced by a file.
    let output = sdkt_isolated(dir.path())
        .args([
            "diff",
            "--old-contract",
            OLD_CONTRACT_ID,
            "--old-wasm",
            "old.wasm",
            "--new-wasm",
            "new.wasm",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "--old-contract vs --old-wasm must be a usage error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--old-contract") && stderr.contains("--old-wasm"),
        "conflict must name both flags, got: {stderr}"
    );

    // --new-contract vs --new-wasm, other side sourced by a file.
    let output = sdkt_isolated(dir.path())
        .args([
            "diff",
            "--old-wasm",
            "old.wasm",
            "--new-contract",
            OLD_CONTRACT_ID,
            "--new-wasm",
            "new.wasm",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "--new-contract vs --new-wasm must be a usage error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--new-contract") && stderr.contains("--new-wasm"),
        "conflict must name both flags, got: {stderr}"
    );
}

#[test]
fn diff_side_without_source_is_usage_error() {
    let dir = tempdir().unwrap();
    // No old-side source at all.
    let output = sdkt_isolated(dir.path())
        .args(["diff", "--new-wasm", "new.wasm"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "missing old side must exit 2"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("old side"),
        "missing side named, got: {stderr}"
    );

    // No sources at all.
    let output = sdkt_isolated(dir.path()).arg("diff").output().unwrap();
    assert_eq!(output.status.code(), Some(2), "bare diff must exit 2");
}

#[test]
fn diff_contract_without_network_is_usage_error() {
    let dir = tempdir().unwrap();
    let (new_contract_id, _) = new_contract_id();
    // No --network / --rpc-url / --network-profile anywhere: the fetch must
    // not silently fall back to the built-in testnet default.
    let output = sdkt_isolated(dir.path())
        .args([
            "diff",
            "--old-contract",
            OLD_CONTRACT_ID,
            "--new-contract",
            &new_contract_id,
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "contract side without network must exit 2"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("require a network"),
        "network requirement named, got: {stderr}"
    );
}

#[test]
fn diff_unknown_contract_fails_with_not_found_not_panic() {
    let dir = tempdir().unwrap();
    let unknown = unknown_contract_id();
    let output = diff_against_mock(
        dir.path(),
        &[
            "--old-contract",
            &unknown,
            "--new-wasm",
            &fixture("us_new.wasm"),
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "unknown contract must fail, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "clear not-found error, got: {stderr}"
    );
    assert!(
        stderr.contains(&unknown),
        "contract id named, got: {stderr}"
    );
}

#[test]
fn diff_upgrade_safety_with_contract_sides() {
    let dir = tempdir().unwrap();
    let (new_contract_id, _) = new_contract_id();
    let output = diff_against_mock(
        dir.path(),
        &[
            "--old-contract",
            OLD_CONTRACT_ID,
            "--new-contract",
            &new_contract_id,
            "--upgrade-safety",
        ],
    );
    assert!(
        output.status.success(),
        "upgrade-safety with contract sides must succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Upgrade Safety") && stdout.contains("Compatible:"),
        "verdict printed, got: {stdout}"
    );
}
