use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

const TEST_WASM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_old.wasm");

#[test]
fn generate_client_writes_file() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("client.rs");
    sdkt()
        .args(["generate", "client", TEST_WASM, "-o"])
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("Generated client"));
    let code = fs::read_to_string(&out).unwrap();
    assert!(code.contains("pub struct TransferCall"));
    assert!(code.contains("pub struct MintCall"));
}

#[test]
fn generate_client_prints_to_stdout() {
    let assert = sdkt().args(["generate", "client", TEST_WASM]).assert();
    assert
        .success()
        .stdout(predicate::str::contains("pub struct TransferCall"))
        .stdout(predicate::str::contains("pub fn contract_functions()"));
}

#[test]
fn generate_client_rejects_missing_file() {
    sdkt()
        .args(["generate", "client", "/nonexistent.wasm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read WASM"));
}

#[test]
fn generate_client_rejects_non_contract_wasm() {
    let tmp = TempDir::new().unwrap();
    let plain = tmp.path().join("plain.wasm");
    fs::write(&plain, b"\x00asm\x01\x00\x00\x00").unwrap();
    sdkt()
        .args(["generate", "client"])
        .arg(&plain)
        .assert()
        .failure()
        .stderr(predicate::str::contains("No contractspecv0 section found"));
}

#[test]
fn generate_client_output_is_deterministic() {
    let tmp = TempDir::new().unwrap();
    let out1 = tmp.path().join("a.rs");
    let out2 = tmp.path().join("b.rs");
    sdkt()
        .args(["generate", "client", TEST_WASM, "-o"])
        .arg(&out1)
        .assert()
        .success();
    sdkt()
        .args(["generate", "client", TEST_WASM, "-o"])
        .arg(&out2)
        .assert()
        .success();
    let a = fs::read_to_string(&out1).unwrap();
    let b = fs::read_to_string(&out2).unwrap();
    assert_eq!(a, b, "generated output must be deterministic");
}

#[test]
fn generate_client_help() {
    sdkt()
        .args(["generate", "client", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("typed Rust client"))
        .stdout(predicate::str::contains("--output"))
        .stdout(predicate::str::contains("--skip-unsupported"));
}

#[test]
fn generate_help_lists_client() {
    sdkt()
        .args(["generate", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("client"));
}

#[test]
fn generate_client_skip_unsupported_on_supported_contract_matches_default() {
    let tmp = TempDir::new().unwrap();
    let out_default = tmp.path().join("default.rs");
    let out_skip = tmp.path().join("skip.rs");

    sdkt()
        .args(["generate", "client", TEST_WASM, "-o"])
        .arg(&out_default)
        .assert()
        .success();

    sdkt()
        .args(["generate", "client", TEST_WASM, "--skip-unsupported", "-o"])
        .arg(&out_skip)
        .assert()
        .success();

    let default_code = fs::read_to_string(&out_default).unwrap();
    let skip_code = fs::read_to_string(&out_skip).unwrap();
    assert_eq!(
        default_code, skip_code,
        "output with --skip-unsupported must be identical when contract has no unsupported types"
    );
    assert!(!skip_code.contains("Skipped"));
}

#[test]
fn generate_client_compiles_with_rustc() {
    let tmp = TempDir::new().unwrap();
    let client_rs = tmp.path().join("client.rs");

    sdkt()
        .args(["generate", "client", TEST_WASM, "--skip-unsupported", "-o"])
        .arg(&client_rs)
        .assert()
        .success();

    let status = std::process::Command::new("rustc")
        .args(["--crate-type", "lib", "--emit", "metadata"])
        .arg(&client_rs)
        .arg("--out-dir")
        .arg(tmp.path())
        .status()
        .expect("failed to run rustc");
    assert!(
        status.success(),
        "generated client code must compile with rustc"
    );
}


// ---------------------------------------------------------------------------
// Issue #512: `sdkt generate client --contract` tests (hermetic mock RPC)
// ---------------------------------------------------------------------------

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use stellar_xdr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ContractId, ExtensionPoint, Hash, LedgerEntry, LedgerEntryData,
    LedgerEntryExt, Limited, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};

const CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const CONTRACT_ID_HEX: &str = "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c";
const WASM_HASH_HEX: &str = "60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02";

fn encode_ledger_entry(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut l = Limited::new(&mut buf, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

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

fn start_mock_contract_rpc(serve_code_entry: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let contract_data_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(CONTRACT_ID_HEX.to_string())).unwrap();
    let contract_code_key =
        encode_ledger_key(&LedgerKeyParams::ContractCode(WASM_HASH_HEX.to_string())).unwrap();

    let mut wasm_hash = [0u8; 32];
    hex::decode_to_slice(WASM_HASH_HEX, &mut wasm_hash).unwrap();

    let wasm_bytes = fs::read(TEST_WASM).unwrap();
    let inspect_xdr = contract_data_entry_xdr(wasm_hash);
    let code_xdr = contract_code_entry_xdr(wasm_hash, &wasm_bytes);

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            let body = if req.contains(""getLedgerEntries"") {
                if req.as_str().contains(contract_data_key.as_str()) {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{contract_data_key}","xdr":"{inspect_xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                    )
                } else if serve_code_entry && req.as_str().contains(contract_code_key.as_str()) {
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
                "HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {}
Connection: close

{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    thread::sleep(Duration::from_millis(40));
    url
}

#[test]
fn generate_client_from_contract_matches_local_wasm_byte_for_byte() {
    let tmp = TempDir::new().unwrap();
    let local_out = tmp.path().join("local_client.rs");
    let contract_out = tmp.path().join("contract_client.rs");
    let rpc_url = start_mock_contract_rpc(true);

    sdkt()
        .args(["generate", "client", TEST_WASM, "-o"])
        .arg(&local_out)
        .assert()
        .success();

    sdkt()
        .args([
            "generate",
            "client",
            "--contract",
            CONTRACT_ID,
            "--rpc-url",
            &rpc_url,
            "-o",
        ])
        .arg(&contract_out)
        .assert()
        .success();

    let local_bytes = fs::read(&local_out).unwrap();
    let contract_bytes = fs::read(&contract_out).unwrap();
    assert_eq!(
        local_bytes, contract_bytes,
        "--contract generated client must be byte-for-byte identical to local WASM client"
    );

    // Also verify stdout output matches byte-for-byte when -o is omitted.
    let stdout_out = sdkt()
        .args([
            "generate",
            "client",
            "--contract",
            CONTRACT_ID,
            "--rpc-url",
            &rpc_url,
        ])
        .output()
        .unwrap();
    assert!(stdout_out.status.success());
    assert_eq!(stdout_out.stdout, local_bytes);
}

#[test]
fn generate_client_contract_and_positional_wasm_conflict() {
    sdkt()
        .args([
            "generate",
            "client",
            TEST_WASM,
            "--contract",
            CONTRACT_ID,
        ])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn generate_client_unknown_contract_fails_cleanly_without_writing_file() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("should_not_exist.rs");
    let rpc_url = start_mock_contract_rpc(true);
    let unknown = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";

    sdkt()
        .args([
            "generate",
            "client",
            "--contract",
            unknown,
            "--rpc-url",
            &rpc_url,
            "-o",
        ])
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot inspect contract"));
    assert!(!out.exists(), "no output file should be written on failure");
}

#[test]
fn generate_client_missing_contract_code_fails_cleanly_without_writing_file() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("should_not_exist_no_code.rs");
    let rpc_url = start_mock_contract_rpc(false);

    sdkt()
        .args([
            "generate",
            "client",
            "--contract",
            CONTRACT_ID,
            "--rpc-url",
            &rpc_url,
            "-o",
        ])
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot fetch WASM for contract"));
    assert!(!out.exists(), "no output file should be written when contract code is missing");
}

#[test]
fn generate_client_unreachable_network_fails_cleanly_without_writing_file() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("should_not_exist_offline.rs");

    sdkt()
        .args([
            "generate",
            "client",
            "--contract",
            CONTRACT_ID,
            "--rpc-url",
            "http://127.0.0.1:1",
            "-o",
        ])
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot inspect contract"));
    assert!(!out.exists(), "no output file should be written when network is unreachable");
}
