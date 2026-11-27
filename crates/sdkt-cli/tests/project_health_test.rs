//! Integration tests for `sdkt project health` rollup subcommand.
//!
//! Verifies:
//! - Empty deployment records exit 0 and emit "No deployments recorded" (pretty) or empty JSON.
//! - Rollup ordering: critical and not_found first, then at_risk, then healthy.
//! - Per-row columns: alias, contract ID, health verdict, minimum TTL, expiring-entry count, short WASM hash.
//! - `--fail-on critical` (default) fails on not_found, critical, or error; passes on at_risk and healthy.
//! - `--fail-on at_risk` fails on at_risk, critical, not_found, or error; passes on all-healthy.
//! - JSON format shape: contains per-contract entries and summary counts that add up to total records.
//! - One record's failure (not found or RPC error) does not abort remaining checks.
//! - Invalid `--fail-on` value exits with usage error (code 2).

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_core::deployment::{DeploymentRecord, DeploymentRecordFile, DEPLOYMENT_RECORD_FILE};
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::thread;
use stellar_xdr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ContractId, ExtensionPoint,
    Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limited, Limits, ScAddress,
    ScContractInstance, ScVal, WriteXdr,
};
use tempfile::TempDir;

const ID_HEALTHY_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const ID_AT_RISK_HEX: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const ID_MISSING_HEX: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const ID_ERROR_HEX: &str = "4444444444444444444444444444444444444444444444444444444444444444";

fn contract_strkey(hex_id: &str) -> String {
    let mut id = [0u8; 32];
    hex::decode_to_slice(hex_id, &mut id).unwrap();
    ScAddress::Contract(ContractId(Hash(id))).to_string()
}

fn encode_ledger_entry(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    let mut l = Limited::new(&mut cursor, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

fn instance_xdr(hex_id: &str, on_chain_hash: &[u8; 32]) -> String {
    let mut id = [0u8; 32];
    hex::decode_to_slice(hex_id, &mut id).unwrap();
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

fn mock_rollup_rpc() -> String {
    let key_healthy =
        encode_ledger_key(&LedgerKeyParams::ContractData(ID_HEALTHY_HEX.to_string())).unwrap();
    let key_at_risk =
        encode_ledger_key(&LedgerKeyParams::ContractData(ID_AT_RISK_HEX.to_string())).unwrap();
    let key_error =
        encode_ledger_key(&LedgerKeyParams::ContractData(ID_ERROR_HEX.to_string())).unwrap();

    let hash_healthy = [0x11u8; 32];
    let hash_at_risk = [0x22u8; 32];

    let entry_healthy = instance_xdr(ID_HEALTHY_HEX, &hash_healthy);
    let entry_at_risk = instance_xdr(ID_AT_RISK_HEX, &hash_at_risk);

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

            let response = if body.contains(&key_error) {
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "error": {
                        "code": -32603,
                        "message": "simulated RPC node failure"
                    }
                })
                .to_string()
            } else if body.contains("\"getLatestLedger\"") {
                r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":28,"sequence":10000}}"#.to_string()
            } else if body.contains(&key_healthy) {
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": { "entries": [{
                        "key": key_healthy, "xdr": entry_healthy,
                        "lastModifiedLedgerSeq": 1, "liveUntilLedgerSeq": 1000000
                    }], "latestLedger": 10000 }
                })
                .to_string()
            } else if body.contains(&key_at_risk) {
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": { "entries": [{
                        "key": key_at_risk, "xdr": entry_at_risk,
                        "lastModifiedLedgerSeq": 1, "liveUntilLedgerSeq": 10500
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

fn sdkt(project: &Path, network_store: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(project)
        .env("SDKT_NETWORK_DIR", network_store)
        .env("SDKT_IDENTITY_DIR", project.join("identity"));
    cmd
}

fn record(alias: &str, contract_id: &str, network: &str) -> DeploymentRecord {
    DeploymentRecord {
        contract_id: contract_id.to_string(),
        wasm_hash: format!("hash-{alias}"),
        network: network.to_string(),
        timestamp: 1_700_000_000,
        salt: Some(format!("salt-{alias}")),
    }
}

fn write_records(project: &Path, entries: &[(&str, &str, &str)]) {
    let mut file = DeploymentRecordFile::default();
    for (scope, alias, contract_id) in entries {
        file.set_record(scope, alias, record(alias, contract_id, scope));
    }
    file.write(project.join(DEPLOYMENT_RECORD_FILE))
        .expect("write deployment records");
}

#[test]
fn project_health_no_recorded_deployments_exits_zero() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    // Pretty mode: prints "No deployments recorded" and exits 0.
    sdkt(project.path(), network_store.path())
        .args(["project", "health"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No deployments recorded"));

    // JSON mode: parses and summary counts are all zero.
    let output = sdkt(project.path(), network_store.path())
        .args(["project", "health", "--format", "json"])
        .output()
        .expect("runs");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["summary"]["total"], 0);
    assert_eq!(parsed["summary"]["healthy"], 0);
    assert_eq!(parsed["summary"]["at_risk"], 0);
    assert_eq!(parsed["summary"]["critical"], 0);
    assert_eq!(parsed["summary"]["not_found"], 0);
    assert_eq!(parsed["contracts"], serde_json::json!([]));
}

#[test]
fn project_health_rollup_ordering_and_fail_on_default() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();
    let rpc_url = mock_rollup_rpc();

    let cid_healthy = contract_strkey(ID_HEALTHY_HEX);
    let cid_at_risk = contract_strkey(ID_AT_RISK_HEX);
    let cid_missing = contract_strkey(ID_MISSING_HEX);

    write_records(
        project.path(),
        &[
            ("testnet", "token", &cid_healthy),
            ("testnet", "vault", &cid_at_risk),
            ("testnet", "ghost", &cid_missing),
        ],
    );

    // Default --fail-on critical must fail (exit 1) because "ghost" is not_found.
    let output = sdkt(project.path(), network_store.path())
        .args(["project", "health", "--rpc-url", &rpc_url])
        .output()
        .expect("runs");
    assert_eq!(output.status.code(), Some(1));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Project health rollup (network: testnet):"));
    assert!(stdout.contains("ALIAS"));
    assert!(stdout.contains("CONTRACT ID"));
    assert!(stdout.contains("HEALTH"));
    assert!(stdout.contains("MIN TTL"));
    assert!(stdout.contains("EXPIRING"));
    assert!(stdout.contains("WASM HASH"));

    // Verify ordering: ghost (not_found) must appear before vault (at_risk),
    // which must appear before token (healthy).
    let idx_ghost = stdout.find("ghost").expect("contains ghost");
    let idx_vault = stdout.find("vault").expect("contains vault");
    let idx_token = stdout.find("token").expect("contains token");

    assert!(
        idx_ghost < idx_vault,
        "not_found contract (ghost) must precede at_risk (vault)"
    );
    assert!(
        idx_vault < idx_token,
        "at_risk contract (vault) must precede healthy (token)"
    );

    // Verify row contents:
    assert!(stdout.contains("not_found"));
    assert!(stdout.contains("at_risk"));
    assert!(stdout.contains("healthy"));
    assert!(stdout.contains("990000")); // min TTL for token
    assert!(stdout.contains("500")); // min TTL for vault
}

#[test]
fn project_health_fail_on_thresholds() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();
    let rpc_url = mock_rollup_rpc();

    let cid_healthy = contract_strkey(ID_HEALTHY_HEX);
    let cid_at_risk = contract_strkey(ID_AT_RISK_HEX);

    write_records(
        project.path(),
        &[
            ("testnet", "token", &cid_healthy),
            ("testnet", "vault", &cid_at_risk),
        ],
    );

    // Workspace with one healthy and one at_risk:
    // With default --fail-on critical: exits 0 (at_risk is below critical threshold).
    sdkt(project.path(), network_store.path())
        .args([
            "project",
            "health",
            "--rpc-url",
            &rpc_url,
            "--fail-on",
            "critical",
        ])
        .assert()
        .success();

    // With --fail-on at_risk: exits 1 (vault is at_risk).
    sdkt(project.path(), network_store.path())
        .args([
            "project",
            "health",
            "--rpc-url",
            &rpc_url,
            "--fail-on",
            "at_risk",
        ])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn project_health_all_healthy_exits_zero() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();
    let rpc_url = mock_rollup_rpc();

    let cid_healthy = contract_strkey(ID_HEALTHY_HEX);

    write_records(project.path(), &[("testnet", "token", &cid_healthy)]);

    // With --fail-on critical: exits 0.
    sdkt(project.path(), network_store.path())
        .args([
            "project",
            "health",
            "--rpc-url",
            &rpc_url,
            "--fail-on",
            "critical",
        ])
        .assert()
        .success();

    // With --fail-on at_risk: exits 0.
    sdkt(project.path(), network_store.path())
        .args([
            "project",
            "health",
            "--rpc-url",
            &rpc_url,
            "--fail-on",
            "at_risk",
        ])
        .assert()
        .success();
}

#[test]
fn project_health_json_format_shape() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();
    let rpc_url = mock_rollup_rpc();

    let cid_healthy = contract_strkey(ID_HEALTHY_HEX);
    let cid_at_risk = contract_strkey(ID_AT_RISK_HEX);
    let cid_missing = contract_strkey(ID_MISSING_HEX);

    write_records(
        project.path(),
        &[
            ("testnet", "token", &cid_healthy),
            ("testnet", "vault", &cid_at_risk),
            ("testnet", "ghost", &cid_missing),
        ],
    );

    let output = sdkt(project.path(), network_store.path())
        .args([
            "project",
            "health",
            "--rpc-url",
            &rpc_url,
            "--format",
            "json",
        ])
        .output()
        .expect("runs");
    assert_eq!(output.status.code(), Some(1));

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["network"], "testnet");

    // Summary counts add up to record count (3):
    let healthy_cnt = parsed["summary"]["healthy"].as_u64().unwrap();
    let at_risk_cnt = parsed["summary"]["at_risk"].as_u64().unwrap();
    let critical_cnt = parsed["summary"]["critical"].as_u64().unwrap();
    let not_found_cnt = parsed["summary"]["not_found"].as_u64().unwrap();
    let total_cnt = parsed["summary"]["total"].as_u64().unwrap();

    assert_eq!(healthy_cnt, 1);
    assert_eq!(at_risk_cnt, 1);
    assert_eq!(critical_cnt, 0);
    assert_eq!(not_found_cnt, 1);
    assert_eq!(total_cnt, 3);
    assert_eq!(
        healthy_cnt + at_risk_cnt + critical_cnt + not_found_cnt,
        total_cnt
    );

    // Per-contract entries array:
    let contracts = parsed["contracts"].as_array().unwrap();
    assert_eq!(contracts.len(), 3);

    // Sorted worst-first: ghost (not_found) first, then vault (at_risk), then token (healthy)
    assert_eq!(contracts[0]["alias"], "ghost");
    assert_eq!(contracts[0]["health"], "not_found");
    assert!(!contracts[0]["error"].is_null());

    assert_eq!(contracts[1]["alias"], "vault");
    assert_eq!(contracts[1]["health"], "at_risk");
    assert_eq!(contracts[1]["minimum_ttl"], 500);
    assert_eq!(contracts[1]["expiring_entries_count"], 1);
    assert_eq!(contracts[1]["short_wasm_hash"], "222222222222");

    assert_eq!(contracts[2]["alias"], "token");
    assert_eq!(contracts[2]["health"], "healthy");
    assert_eq!(contracts[2]["minimum_ttl"], 990000);
    assert_eq!(contracts[2]["expiring_entries_count"], 0);
    assert_eq!(contracts[2]["short_wasm_hash"], "111111111111");
}

#[test]
fn project_health_rpc_error_continues_remaining_checks_and_fails() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();
    let rpc_url = mock_rollup_rpc();

    let cid_healthy = contract_strkey(ID_HEALTHY_HEX);
    let cid_error = contract_strkey(ID_ERROR_HEX);

    write_records(
        project.path(),
        &[
            ("testnet", "broken", &cid_error),
            ("testnet", "token", &cid_healthy),
        ],
    );

    let output = sdkt(project.path(), network_store.path())
        .args([
            "project",
            "health",
            "--rpc-url",
            &rpc_url,
            "--format",
            "json",
        ])
        .output()
        .expect("runs");
    assert_eq!(output.status.code(), Some(1));

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["summary"]["error"], 1);
    assert_eq!(parsed["summary"]["healthy"], 1);
    assert_eq!(parsed["summary"]["total"], 2);

    let contracts = parsed["contracts"].as_array().unwrap();
    assert_eq!(contracts.len(), 2);
    // broken (error) is worst tier, so sorted first
    assert_eq!(contracts[0]["alias"], "broken");
    assert_eq!(contracts[0]["health"], "error");
    assert_eq!(contracts[1]["alias"], "token");
    assert_eq!(contracts[1]["health"], "healthy");
}

#[test]
fn project_health_invalid_fail_on_exits_usage_error() {
    let project = TempDir::new().unwrap();
    let network_store = TempDir::new().unwrap();

    sdkt(project.path(), network_store.path())
        .args(["project", "health", "--fail-on", "bogus"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("invalid --fail-on value"));
}
