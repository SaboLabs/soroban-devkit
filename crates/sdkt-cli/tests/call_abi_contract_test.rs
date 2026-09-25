//! — `call --abi-contract` tests (hermetic).
//!
//! `sdkt call --abi-contract <id>` fetches a deployed contract's on-chain WASM
//! via `inspect_contract` -> `get_wasm_bytecode`, parses it to a
//! `ContractSpec`, and decodes the simulated call result using
//! `decode_with_abi` — the same result the local `--abi` path produces.
//!
//! All network interaction is served by an in-process mock JSON-RPC server that
//! routes per-method (`simulateTransaction`) and per-key (`getLedgerEntries` for
//! the contract-data and contract-code ledger entries it must return). No live
//! Testnet is required.

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use predicates::prelude::*;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use stellar_xdr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ContractId, ExtensionPoint, Hash, LedgerEntry, LedgerEntryData,
    LedgerEntryExt, LedgerEntryType, Limited, Limits, ScAddress, ScContractInstance, ScVal,
    WriteXdr,
};
use tempfile::tempdir;

/// A real contractspecv0 WASM fixture declaring functions that return `u32`.
static WASM_WITH_RETURN: &[u8] = include_bytes!("fixtures/us_new.wasm");

/// ScVal::U32(42) in base64 XDR.
const MOCK_SCVAL_U32_42: &str = "AAAAAwAAACo=";

/// A deployed-testnet contract StrKey; its hash is `09ba7d2a...06c`.
const CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

/// On-chain WASM hash the mock serves for `CONTRACT_ID`.
const WASM_HASH_HEX: &str = "60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02";

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

/// Build the base64 `LedgerEntry` for a `CONTRACT_CODE` entry carrying `code` — the
/// shape `get_wasm_bytecode` reads back into raw bytes.
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

/// Mock JSON-RPC server covering the whole `call --abi-contract` flow:
///
/// - `simulateTransaction` → a result ScVal of `u32(42)`.
/// - `getLedgerEntries` → routed by the encoded ledger key in the request body:
///   - the contract-data key → a `CONTRACT_DATA` entry resolving to `WASM_HASH_HEX`;
///   - the contract-code key → a `CONTRACT_CODE` entry carrying `wasm_bytes`;
///   - any other key → empty entries (contract not found).
fn mock_rpc_server_with_abi(wasm_bytes: &'static [u8]) -> (String, &'static [u8]) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let contract_id_hex = "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c";
    let contract_data_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(contract_id_hex.to_string())).unwrap();
    let contract_code_key =
        encode_ledger_key(&LedgerKeyParams::ContractCode(WASM_HASH_HEX.to_string())).unwrap();

    let mut wasm_hash = [0u8; 32];
    hex::decode_to_slice(WASM_HASH_HEX, &mut wasm_hash).unwrap();

    let inspect_xdr = contract_data_entry_xdr(wasm_hash);
    let code_xdr = contract_code_entry_xdr(wasm_hash, wasm_bytes);

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            let body = if req.contains("\"simulateTransaction\"") {
                format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"results":[{{"xdr":"{MOCK_SCVAL_U32_42}","auth":[]}}],"latestLedger":"12345","events":[],"cost":{{"cpuInsns":"1000","memBytes":"2000"}}}}}}"#
                )
            } else if req.contains("\"getLedgerEntries\"") {
                if req.as_str().contains(contract_data_key.as_str()) {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{contract_data_key}","xdr":"{inspect_xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                    )
                } else if req.as_str().contains(contract_code_key.as_str()) {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{contract_code_key}","xdr":"{code_xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                    )
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
        }
    });

    thread::sleep(Duration::from_millis(50));
    (url, wasm_bytes)
}

#[test]
fn call_help_documents_abi_contract() {
    sdkt_isolated(tempdir().unwrap().path())
        .args(["call", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("abi-contract"));
}

#[test]
fn call_abi_and_abi_contract_are_mutually_exclusive() {
    sdkt_isolated(tempdir().unwrap().path())
        .args([
            "call",
            CONTRACT_ID,
            "hello",
            "--abi",
            "/no/such.wasm",
            "--abi-contract",
            CONTRACT_ID,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "specify only one of --abi or --abi-contract",
        ));
}

#[test]
fn call_abi_contract_decodes_result_from_onchain_wasm() {
    let dir = tempdir().unwrap();
    let (url, wasm_bytes) = mock_rpc_server_with_abi(WASM_WITH_RETURN);
    assert!(!wasm_bytes.is_empty());
    add_mock_profile(dir.path(), &url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "hello",
            "--network-profile",
            "mocknet",
            "--abi-contract",
            CONTRACT_ID,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("u32(42)"));
}

#[test]
fn call_abi_contract_json_output_has_decoded_result() {
    let dir = tempdir().unwrap();
    let (url, _) = mock_rpc_server_with_abi(WASM_WITH_RETURN);
    add_mock_profile(dir.path(), &url);

    let output = sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "hello",
            "--network-profile",
            "mocknet",
            "--abi-contract",
            CONTRACT_ID,
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nOutput: {stdout}"));

    assert!(parsed.get("result").is_some(), "Missing result field");
    assert!(parsed.get("decoded").is_some(), "Missing decoded field");
    assert_eq!(parsed["decoded"]["label"], "u32(42)");
}

#[test]
fn call_abi_contract_missing_contract_is_clear_error() {
    // The mock serves no contract-data entry for this other StrKey, so
    // `inspect_contract` degrades to a controlled "not found" error.
    let dir = tempdir().unwrap();
    let (url, _) = mock_rpc_server_with_abi(WASM_WITH_RETURN);
    add_mock_profile(dir.path(), &url);

    let other = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
    sdkt_isolated(dir.path())
        .args([
            "call",
            other,
            "hello",
            "--network-profile",
            "mocknet",
            "--abi-contract",
            other,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"))
        .stderr(predicate::str::contains("panic").not());
}

#[test]
fn call_abi_contract_offline_is_graceful() {
    // No RPC reachable -> clean failure (no panic), actionable error message.
    let dir = tempdir().unwrap();
    add_mock_profile(dir.path(), "http://127.0.0.1:1");

    sdkt_isolated(dir.path())
        .args([
            "call",
            CONTRACT_ID,
            "hello",
            "--network-profile",
            "mocknet",
            "--abi-contract",
            CONTRACT_ID,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error"))
        .stderr(predicate::str::contains("panic").not());
}

#[test]
fn call_without_abi_preserves_raw_result() {
    // No-ABI behavior is unchanged: the raw base64 XDR result is shown.
    let dir = tempdir().unwrap();
    let (url, _) = mock_rpc_server_with_abi(WASM_WITH_RETURN);
    add_mock_profile(dir.path(), &url);

    sdkt_isolated(dir.path())
        .args(["call", CONTRACT_ID, "hello", "--network-profile", "mocknet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("u32(42)").not());
}

#[test]
fn valid_fixture_is_a_real_contractspec_wasm() {
    // Guard the mock's core assumption: the WASM bytes it serves over
    // `getLedgerEntries` must parse to a ContractSpec, or the whole on-chain ABI
    // path silently produces no spec.
    let spec = sdkt_wasm::parse_contract_spec(WASM_WITH_RETURN)
        .expect("fixture parses as a contractspecv0 WASM");
    assert!(
        !spec.functions.is_empty(),
        "expected a declared function in the fixture"
    );
}

#[test]
fn ledger_key_discriminants_are_contract_variants() {
    // Ensure the mock's ledger keys decode to ContractData / ContractCode so the
    // `getLedgerEntries` routing really discriminates the two on-chain lookups.
    use stellar_xdr::ReadXdr;

    let data_key = encode_ledger_key(&LedgerKeyParams::ContractData(
        "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c".to_string(),
    ))
    .unwrap();
    let code_key =
        encode_ledger_key(&LedgerKeyParams::ContractCode(WASM_HASH_HEX.to_string())).unwrap();

    let decode_key_type = |key: &str| {
        let raw = STANDARD.decode(key).unwrap();
        let mut cursor = std::io::Cursor::new(raw);
        let mut l = Limited::new(&mut cursor, Limits::none());
        stellar_xdr::LedgerKey::read_xdr(&mut l)
            .unwrap()
            .discriminant()
    };

    assert_eq!(decode_key_type(&data_key), LedgerEntryType::ContractData);
    assert_eq!(decode_key_type(&code_key), LedgerEntryType::ContractCode);
    // The two keys genuinely differ so the mock can route between them.
    assert_ne!(data_key, code_key);
}
