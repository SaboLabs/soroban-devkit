use assert_cmd::Command;

/// Minimal valid WASM binary (magic + version 1).
const MINIMAL_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

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

// ── --fail-on flag tests ─────────────────────────────────────────────────────

/// `sdkt health --fail-on` with an unrecognised value exits 1 before making
/// any network request (fail-fast on bad input).
#[test]
fn health_fail_on_invalid_value_rejects_early() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), MINIMAL_WASM).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("health")
        .arg("--contract")
        .arg("CNotARealContractId")
        .arg("--wasm")
        .arg(tmp.path())
        .arg("--fail-on")
        .arg("bogus")
        .assert()
        .failure()
        .stderr(predicates::str::contains("--fail-on must be"));
}

/// Without `--fail-on` an operational error (e.g. invalid contract ID)
/// exits 1, not 2.
#[test]
fn health_flagless_run_does_not_use_fail_on() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), MINIMAL_WASM).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let output = cmd
        .arg("health")
        .arg("--contract")
        .arg("CNotARealContractId")
        .arg("--wasm")
        .arg(tmp.path())
        .output()
        .unwrap();

    // The exit code must be 1 (operational error), NOT 2 (gate).
    assert_eq!(
        output.status.code(),
        Some(1),
        "flagless health must exit 1 for an operational error"
    );
}

/// `sdkt health --help` must document `--fail-on` so users can discover it.
#[test]
fn health_help_documents_fail_on_flag() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.args(["health", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("fail-on"));
}

// ── Mock RPC integration tests for --fail-on gating ─────────────────────────

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use stellar_xdr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ContractId, ExtensionPoint, Hash, LedgerEntry, LedgerEntryData,
    LedgerEntryExt, Limited, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};

static HEALTH_WASM_FIXTURE: &[u8] = include_bytes!("fixtures/us_old.wasm");
const HEALTH_CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const HEALTH_WASM_HASH_HEX: &str =
    "05befa136e7f0829a5051d97b032f355a5e65976397df90b224d141942dce46c";

/// Read one HTTP request in full (headers + body according to Content-Length).
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

fn spawn_mock_health_rpc() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let contract_id_hex = "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c";
    let contract_data_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(contract_id_hex.to_string())).unwrap();
    let contract_code_key = encode_ledger_key(&LedgerKeyParams::ContractCode(
        HEALTH_WASM_HASH_HEX.to_string(),
    ))
    .unwrap();

    let mut wasm_hash = [0u8; 32];
    hex::decode_to_slice(HEALTH_WASM_HASH_HEX, &mut wasm_hash).unwrap();

    let encode_entry = |entry: &LedgerEntry| -> String {
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        entry.write_xdr(&mut l).unwrap();
        STANDARD.encode(&buf)
    };

    let inspect_xdr = encode_entry(&LedgerEntry {
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
    });

    let code_xdr = encode_entry(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(wasm_hash),
            code: HEALTH_WASM_FIXTURE.to_vec().try_into().unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    });

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let req = read_request(&mut sock);

            let body = if req.contains("\"getLatestLedger\"") {
                r#"{"jsonrpc":"2.0","id":1,"result":{"id":"test","sequence":100,"protocolVersion":20}}"#.to_string()
            } else if req.contains("\"getLedgerEntries\"") {
                if req.contains(&contract_data_key) {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{contract_data_key}","xdr":"{inspect_xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                    )
                } else if req.contains(&contract_code_key) {
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
            let _ = sock.flush();
        }
    });

    url
}

/// Flagless health run against a reachable RPC exits 0 even when the verdict is AT_RISK.
#[test]
fn health_flagless_success_exits_zero() {
    let rpc_url = spawn_mock_health_rpc();
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.args([
        "health",
        "--contract",
        HEALTH_CONTRACT_ID,
        "--rpc-url",
        &rpc_url,
    ])
    .assert()
    .success()
    .stdout(predicates::str::contains("Contract Health Report"))
    .stdout(predicates::str::contains("Health      : AT_RISK"));
}

/// `health --fail-on at_risk` exits code 2 when the verdict is AT_RISK,
/// and the full report is printed before exit.
#[test]
fn health_fail_on_at_risk_gates_with_code_2() {
    let rpc_url = spawn_mock_health_rpc();
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.args([
        "health",
        "--contract",
        HEALTH_CONTRACT_ID,
        "--fail-on",
        "at_risk",
        "--rpc-url",
        &rpc_url,
    ])
    .assert()
    .code(2)
    .stdout(predicates::str::contains("Contract Health Report"))
    .stdout(predicates::str::contains("Health      : AT_RISK"));
}

/// `health --fail-on critical` exits 0 when the verdict is only AT_RISK
/// (threshold is not met).
#[test]
fn health_fail_on_critical_does_not_gate_at_risk() {
    let rpc_url = spawn_mock_health_rpc();
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.args([
        "health",
        "--contract",
        HEALTH_CONTRACT_ID,
        "--fail-on",
        "critical",
        "--rpc-url",
        &rpc_url,
    ])
    .assert()
    .success()
    .stdout(predicates::str::contains("Contract Health Report"))
    .stdout(predicates::str::contains("Health      : AT_RISK"));
}

/// `health --fail-on critical` exits code 2 when local WASM mismatches on-chain WASM
/// (which derives CRITICAL health), and the full report is printed before exit.
#[test]
fn health_fail_on_critical_gates_with_code_2() {
    let rpc_url = spawn_mock_health_rpc();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), MINIMAL_WASM).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.args([
        "health",
        "--contract",
        HEALTH_CONTRACT_ID,
        "--wasm",
        tmp.path().to_str().unwrap(),
        "--fail-on",
        "critical",
        "--rpc-url",
        &rpc_url,
    ])
    .assert()
    .code(2)
    .stdout(predicates::str::contains("Contract Health Report"))
    .stdout(predicates::str::contains("Health      : CRITICAL"))
    .stdout(predicates::str::contains("On-chain WASM does NOT match"));
}
