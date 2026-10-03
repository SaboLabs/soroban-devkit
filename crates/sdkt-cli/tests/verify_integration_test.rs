use assert_cmd::Command;

/// Minimal valid WASM binary (magic + version 1).
const MINIMAL_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

#[test]
fn test_cli_verify_missing_contract_arg() {
    // `--contract` is required by clap → failure without touching RPC.
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd.arg("verify").assert();
    assert.failure();
}

#[test]
fn test_cli_verify_invalid_format_arg() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CABCDEFG")
        .arg("--format")
        .arg("bogus")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("Invalid format"));
}

#[test]
fn test_cli_verify_missing_wasm_file() {
    // Missing local file must fail offline with a clear message (no RPC).
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CABCDEFG")
        .arg("--wasm")
        .arg("/nonexistent/path/contract.wasm")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("Error reading WASM"));
}

#[test]
fn test_cli_verify_invalid_wasm() {
    // Invalid local WASM must fail offline (fail-fast before RPC).
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"not a wasm file").unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CABCDEFG")
        .arg("--wasm")
        .arg(tmp.path())
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("not valid WASM"));
}

#[test]
fn test_cli_verify_json_format_accepted() {
    // `--format json` must be parsed; an invalid local WASM still fails
    // offline, proving the JSON path is reachable without a network.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"not a wasm file").unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CABCDEFG")
        .arg("--wasm")
        .arg(tmp.path())
        .arg("--format")
        .arg("json")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("not valid WASM"));
}

#[test]
fn test_cli_verify_onchain_error_path() {
    // With a valid local WASM but a bogus contract id, the command must reach
    // the RPC layer and exit non-zero (offline this surfaces as a network/
    // contract error, not a panic). Exercises the on-chain fetch + error path.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), MINIMAL_WASM).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CNotARealContractId")
        .arg("--wasm")
        .arg(tmp.path())
        .assert();
    assert.failure();
}

// ---- G1 exit-code contract (Problem Hunt #002) -----------------------------
//
// `sdkt verify` must behave as a real verification gate:
//   Verified    -> exit 0
//   OnChainOnly -> exit 0 (no local artifact supplied: nothing to fail)
//   Mismatch    -> exit NON-ZERO (with the report still printed)
//
// Deterministic offline proof against a local mock RPC serving a real
// protocol-valid `LedgerEntry` (ContractInstance with a Wasm executable),
// same harness pattern as the existing contract test suites.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use stellar_xdr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ContractId, ExtensionPoint,
    Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limited, Limits, ScAddress,
    ScContractInstance, ScVal, WriteXdr,
};

/// 32-byte contract id under test + its `C...` strkey.
const TEST_ID_HEX: &str = "7777777777777777777777777777777777777777777777777777777777777777";

/// The `C...` strkey for [`TEST_ID_HEX`] (via the same `ScAddress` Display the
/// CLI uses, so the test exercises the real id path).
fn test_contract_strkey() -> String {
    let mut id = [0u8; 32];
    hex::decode_to_slice(TEST_ID_HEX, &mut id).unwrap();
    ScAddress::Contract(ContractId(Hash(id))).to_string()
}

/// sha256 of `bytes` as a raw 32-byte array.
fn sha256_raw(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn encode_ledger_entry(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    let mut l = Limited::new(&mut cursor, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

/// Instance ledger entry whose executable hash is `on_chain_hash` (raw bytes).
fn instance_xdr(on_chain_hash: &[u8; 32]) -> String {
    let mut id = [0u8; 32];
    hex::decode_to_slice(TEST_ID_HEX, &mut id).unwrap();
    encode_ledger_entry(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(ContractId(Hash(id))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(*on_chain_hash)),
                storage: None,
            }),
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

/// Mock RPC answering only the instance lookup (serves `instance_xdr`);
/// every other key (e.g. the best-effort bytecode fetch) returns no entries.
fn mock_verify_rpc(on_chain_hash: [u8; 32]) -> String {
    let instance_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(TEST_ID_HEX.to_string())).unwrap();
    let entry_xdr = instance_xdr(&on_chain_hash);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let request = read_rpc_request(&mut sock);
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or("");
            let is_instance_lookup = body.contains(&instance_key);
            let response = if body.contains("\"getLatestLedger\"") {
                r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":28,"sequence":10000}}"#.to_string()
            } else if is_instance_lookup {
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": { "entries": [{
                        "key": instance_key, "xdr": entry_xdr,
                        "lastModifiedLedgerSeq": 1, "liveUntilLedgerSeq": 12345
                    }], "latestLedger": 10000 }
                })
                .to_string()
            } else {
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": { "entries": [], "latestLedger": 10000 }
                })
                .to_string()
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

fn write_wasm(bytes: &[u8]) -> tempfile::NamedTempFile {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), bytes).unwrap();
    tmp
}

#[test]
fn verify_match_exits_zero() {
    let local = MINIMAL_WASM;
    let hash: [u8; 32] = sha2::Sha256::digest(local).as_slice().try_into().unwrap();
    let url = mock_verify_rpc(hash);
    let file = write_wasm(local);

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("verify")
        .arg("--contract")
        .arg(test_contract_strkey())
        .arg("--wasm")
        .arg(file.path())
        .arg("--format")
        .arg("json")
        .arg("--rpc-url")
        .arg(url)
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "\"verification_status\": \"Verified\"",
        ));
}

#[test]
fn verify_mismatch_exits_nonzero() {
    // The local artifact is a valid WASM; the deployed code is a DIFFERENT
    // build (its hash is the sha256 of other, equally valid, bytes). The
    // comparison must therefore report Mismatch — and exit non-zero.
    let other: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x0b];
    let on_chain: [u8; 32] = sha256_raw(other);
    assert_ne!(
        on_chain,
        sha256_raw(MINIMAL_WASM),
        "fixture sanity: the two files must hash differently"
    );
    let url = mock_verify_rpc(on_chain);
    let file = write_wasm(MINIMAL_WASM);

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg(test_contract_strkey())
        .arg("--wasm")
        .arg(file.path())
        .arg("--format")
        .arg("json")
        .arg("--rpc-url")
        .arg(url)
        .assert()
        .failure();
    let output = assert.get_output();
    // The report must still be printed (schema unchanged)...
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"verification_status\": \"Mismatch\"")
            && stdout.contains("\"match\": false"),
        "mismatch report must still be emitted: {stdout}"
    );
    // ...and the failure must be actionable on stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does NOT match"),
        "expected actionable mismatch error, got: {stderr}"
    );
}

#[test]
fn verify_onchain_only_without_wasm_exits_zero() {
    // No local artifact supplied: there is nothing to fail on; the hash is
    // simply reported.
    let on_chain: [u8; 32] = sha256_raw(MINIMAL_WASM);
    let url = mock_verify_rpc(on_chain);

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("verify")
        .arg("--contract")
        .arg(test_contract_strkey())
        .arg("--format")
        .arg("json")
        .arg("--rpc-url")
        .arg(url)
        .assert()
        .success()
        .stdout(predicates::str::contains("OnChainOnly"));
}

#[test]
fn verify_rpc_failure_still_exits_nonzero() {
    // Genuine operational errors keep failing: bind-then-drop a port so the
    // RPC endpoint is unreachable without any race with a live server.
    let file = write_wasm(MINIMAL_WASM);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("verify")
        .arg("--contract")
        .arg(test_contract_strkey())
        .arg("--wasm")
        .arg(file.path())
        .arg("--rpc-url")
        .arg(url)
        .assert()
        .failure();
}
