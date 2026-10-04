//! `sdkt health` behavior tests.
//!
//! Offline/negative paths plus two hermetic mock-RPC paths that prove the
//! exit code reflects the verdict: a `critical` posture (deployed WASM does
//! not match the supplied local file) must exit non-zero, while a
//! non-blocking `at_risk` posture must not.

use assert_cmd::Command;
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

/// Minimal valid WASM binary (magic + version 1).
const MINIMAL_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

/// 32-byte contract id under test + its `C...` strkey.
const TEST_ID_HEX: &str = "7777777777777777777777777777777777777777777777777777777777777777";

/// The `C...` strkey for [`TEST_ID_HEX`], built through the same `ScAddress`
/// Display the CLI uses so the test exercises the real id path.
fn test_contract_strkey() -> String {
    let mut id = [0u8; 32];
    hex::decode_to_slice(TEST_ID_HEX, &mut id).unwrap();
    ScAddress::Contract(ContractId(Hash(id))).to_string()
}

fn encode_ledger_entry(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    let mut l = Limited::new(&mut cursor, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

/// Instance ledger entry whose executable hash is `on_chain_hash`.
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

/// Mock RPC that serves the contract instance entry for [`TEST_ID_HEX`] with
/// the given executable hash, and an empty storage report. Storage analysis
/// therefore finds no entries — a non-blocking `at_risk` reason — so the only
/// way to reach `critical` is the WASM mismatch under test.
fn mock_health_rpc(on_chain_hash: [u8; 32]) -> String {
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
            let response = if body.contains("\"getLatestLedger\"") {
                r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":28,"sequence":10000}}"#.to_string()
            } else if body.contains(&instance_key) {
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
fn test_cli_health_missing_contract_arg() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd.arg("health").assert();
    assert.failure();
}

#[test]
fn test_cli_health_invalid_format_arg() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
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
fn test_cli_health_missing_wasm_file() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
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
fn test_cli_health_invalid_wasm() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"not a wasm file").unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
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
fn test_cli_health_json_format_accepted() {
    // --format json must be parsed; an invalid local WASM still fails
    // offline, proving the JSON path is reachable without a network.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"not a wasm file").unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
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
fn test_cli_health_onchain_error_path() {
    // Valid local WASM + bogus contract id → reaches the RPC layer and exits
    // non-zero (offline this surfaces as a network/contract error), exercising
    // the on-chain fetch + error branch.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), MINIMAL_WASM).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
        .arg("--contract")
        .arg("CNotARealContractId")
        .arg("--wasm")
        .arg(tmp.path())
        .assert();
    assert.failure();
}

#[test]
fn health_critical_verdict_exits_nonzero() {
    // Deployed code hashes differently from the supplied local artifact, so
    // `derive_verdict` yields `critical`. A critical posture is a real failure
    // and must be observable from the exit code alone — otherwise CI and
    // agents read a green run over a mismatched artifact.
    let other: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x0b];
    let on_chain: [u8; 32] = Sha256::digest(other).into();
    assert_ne!(
        on_chain,
        <[u8; 32]>::from(Sha256::digest(MINIMAL_WASM)),
        "fixture sanity: the two artifacts must hash differently"
    );
    let url = mock_health_rpc(on_chain);
    let file = write_wasm(MINIMAL_WASM);

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
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
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).clone();
    assert!(
        stdout.contains("\"health\": \"critical\""),
        "expected a critical verdict in the report, got: {stdout}"
    );
}

#[test]
fn health_at_risk_verdict_still_exits_zero() {
    // Same mock, but no local artifact to compare: verification is skipped and
    // the only reason is the empty storage report, which is `at_risk`. That
    // posture keeps its existing non-blocking meaning (exit 0), matching how
    // `release-assurance` maps `at_risk` to REVIEW rather than FAIL.
    let url = mock_health_rpc([0x11u8; 32]);

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
        .arg("--contract")
        .arg(test_contract_strkey())
        .arg("--format")
        .arg("json")
        .arg("--rpc-url")
        .arg(url)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).clone();
    assert!(
        stdout.contains("\"health\": \"at_risk\""),
        "expected an at_risk verdict in the report, got: {stdout}"
    );
}
