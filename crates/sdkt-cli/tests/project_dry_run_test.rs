//! Integration tests for `sdkt project deploy --dry-run` (issue #538).
//!
//! Every network interaction is served by an in-process mock JSON-RPC server
//! (same pattern as `project_deploy_record_test.rs`), so no live Testnet is
//! required. The tests exercise:
//!
//! - a dry run prints the full deployment plan in dependency order and
//!   submits nothing (zero `sendTransaction` / `simulateTransaction` calls),
//! - `--format json` output parses and carries one object per contract,
//! - `--skip-deployed` marks an already-deployed alias as skipped instead of
//!   predicting a fresh address for it,
//! - the predicted addresses match the addresses a real deploy (against the
//!   same mock, with the same salt) records in `.sdkt-deployments.json`.

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use predicates::prelude::*;
use sdkt_core::deployment::{DeploymentRecord, DeploymentRecordFile, DEPLOYMENT_RECORD_FILE};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use stellar_xdr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ContractId, ExtensionPoint,
    Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limited, Limits, ScAddress,
    ScContractInstance, ScVal, WriteXdr,
};
use tempfile::TempDir;

/// A real contractspecv0 WASM fixture (also used by the ABI contract tests).
static CONTRACT_WASM: &[u8] = include_bytes!("fixtures/us_new.wasm");

/// LedgerEntry XDR for an account with seq_num 41 (so next sequence = 42).
const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";

/// SorobanTransactionData XDR: empty footprint, 1000 instructions, 150 stroops
/// resource fee.
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

/// Fixed salt (20 bytes) so predicted and recorded addresses are comparable.
const SALT_HEX: &str = "000102030405060708090a0b0c0d0e0f10111213";

/// A synthetic contract ID that exists only in the seeded record file.
const RECORDED_CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

fn sdkt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

/// Set up a 2-contract project (token ← vault) with real WASM artifacts under
/// each contract's release target directory, so `resolve_project` resolves a
/// valid deploy order without running `sdkt build`.
fn setup_project(dir: &Path) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"

[contracts.vault]
path = "contracts/vault"
depends_on = ["token"]
"#;
    std::fs::write(dir.join(".sdkt.toml"), manifest).unwrap();

    for alias in ["token", "vault"] {
        let target = dir
            .join("contracts")
            .join(alias)
            .join("target")
            .join(sdkt_core::build::WASM_BUILD_TARGET)
            .join("release");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join(format!("{alias}.wasm")), CONTRACT_WASM).unwrap();
    }
}

/// Create a usable signing identity for `project deploy`, which resolves the
/// hardcoded `default` identity via the `sdkt identity default` command.
fn generate_default_identity(dir: &Path) {
    sdkt(dir)
        .args(["identity", "generate", "alice"])
        .assert()
        .success();
    sdkt(dir)
        .args(["identity", "default", "alice"])
        .assert()
        .success();
}

/// Base64 XDR for a live contract instance entry — the shape `contract_exists`
/// treats as "the recorded contract is on-chain".
fn contract_instance_entry_xdr() -> String {
    let entry = LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash([0; 32])),
                storage: None,
            }),
        }),
        ext: LedgerEntryExt::V0,
    };
    let mut buf = Vec::new();
    let mut l = Limited::new(&mut buf, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

fn read_full_request(sock: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = sock.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data).to_string();
        if let Some(header_end) = text.find("\r\n\r\n") {
            let content_length = text[..header_end]
                .lines()
                .filter_map(|l| l.split_once(':'))
                .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= header_end + 4 + content_length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).to_string()
}

fn extract_jsonrpc_method(req: &str) -> Option<String> {
    let body_start = req.find("\r\n\r\n")?;
    let body = &req[body_start + 4..];
    let val: serde_json::Value = serde_json::from_str(body).ok()?;
    val.get("method")?.as_str().map(ToString::to_string)
}

fn parse_get_ledger_keys(req: &str) -> Vec<String> {
    let body_start = match req.find("\r\n\r\n") {
        Some(i) => i + 4,
        None => return Vec::new(),
    };
    let body = &req[body_start..];
    let Ok(val) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    val.get("params")
        .and_then(|p| p.get("keys"))
        .and_then(|k| k.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|k| k.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn is_account_key(key: &str) -> bool {
    matches!(
        sdkt_xdr::decode_ledger_key(key),
        Ok(stellar_xdr::LedgerKey::Account(_))
    )
}

/// Mock JSON-RPC server that counts every mutating call, so a dry run can be
/// proven to submit nothing.
///
/// - `getLedgerEntries`: the account key always returns an account entry; the
///   contract instance key returns a live contract entry when `instance_live`
///   is true, empty entries otherwise.
/// - `simulateTransaction` / `sendTransaction`: counted, then answered with
///   success (so a real deploy against this mock succeeds).
/// - `getTransaction` → SUCCESS.
struct MockServer {
    url: String,
    sim_count: Arc<AtomicUsize>,
    send_count: Arc<AtomicUsize>,
}

impl MockServer {
    fn start(instance_live: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let sim_count = Arc::new(AtomicUsize::new(0));
        let send_count = Arc::new(AtomicUsize::new(0));
        let sim_count_thread = sim_count.clone();
        let send_count_thread = send_count.clone();
        let instance_xdr = contract_instance_entry_xdr();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let mut sock = match conn {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let req = read_full_request(&mut sock);
                if req.is_empty() {
                    continue;
                }
                let method = extract_jsonrpc_method(&req);

                let body = match method.as_deref() {
                    Some("getLedgerEntries") => {
                        let keys = parse_get_ledger_keys(&req);
                        if keys.iter().any(|k| is_account_key(k)) {
                            format!(
                                r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                            )
                        } else if instance_live {
                            format!(
                                r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{instance_xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                            )
                        } else {
                            r#"{"jsonrpc":"2.0","id":1,"result":{"entries":[],"latestLedger":100}}"#
                                .to_string()
                        }
                    }
                    Some("simulateTransaction") => {
                        sim_count_thread.fetch_add(1, Ordering::SeqCst);
                        format!(
                            r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                        )
                    }
                    Some("sendTransaction") => {
                        send_count_thread.fetch_add(1, Ordering::SeqCst);
                        r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeefcafe","status":"PENDING","latestLedger":"100"}}"#
                            .to_string()
                    }
                    Some("getTransaction") => {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg=="}}"#
                            .to_string()
                    }
                    _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                        .to_string(),
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        Self {
            url,
            sim_count,
            send_count,
        }
    }

    fn simulate_count(&self) -> usize {
        self.sim_count.load(Ordering::SeqCst)
    }

    fn send_count(&self) -> usize {
        self.send_count.load(Ordering::SeqCst)
    }
}

fn deploy_base_args(mock: &MockServer) -> Vec<String> {
    vec![
        "project".into(),
        "--rpc-url".into(),
        mock.url.clone(),
        "--network-passphrase".into(),
        "Test SDF Network ; September 2015".into(),
        "deploy".into(),
    ]
}

fn wasm_hash_hex() -> String {
    hex::encode(Sha256::digest(CONTRACT_WASM))
}

#[test]
fn dry_run_prints_plan_and_submits_nothing() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project(project_dir);
    generate_default_identity(project_dir);
    let mock = MockServer::start(false);

    let mut args = deploy_base_args(&mock);
    args.push("--dry-run".into());
    args.push("--salt".into());
    args.push(SALT_HEX.into());
    let output = sdkt(project_dir)
        .current_dir(project_dir)
        .args(&args)
        .assert()
        .success()
        .stdout(predicate::str::contains("token"))
        .stdout(predicate::str::contains("vault"))
        .stdout(predicate::str::contains("No transactions submitted."))
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    // Dependency order: token before vault.
    let token_pos = stdout.find("token").unwrap();
    let vault_pos = stdout.find("vault").unwrap();
    assert!(token_pos < vault_pos, "plan must be in deploy order");
    // Both contracts carry the WASM hash and the salt.
    assert!(stdout.contains(&wasm_hash_hex()), "plan shows WASM hash");
    assert!(stdout.contains(SALT_HEX), "plan shows the salt");
    assert!(stdout.contains("2 contract(s) to deploy, 0 skipped"));

    // Nothing was simulated, nothing was submitted, and no record was written.
    assert_eq!(mock.simulate_count(), 0, "dry run must not simulate");
    assert_eq!(mock.send_count(), 0, "dry run must not submit");
    assert!(
        !project_dir.join(DEPLOYMENT_RECORD_FILE).exists(),
        "dry run must not write a deployment record"
    );
}

#[test]
fn dry_run_json_output_parses_with_one_object_per_contract() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project(project_dir);
    generate_default_identity(project_dir);
    let mock = MockServer::start(false);

    let mut args = deploy_base_args(&mock);
    args.push("--dry-run".into());
    args.push("--salt".into());
    args.push(SALT_HEX.into());
    args.push("--format".into());
    args.push("json".into());
    let output = sdkt(project_dir)
        .current_dir(project_dir)
        .args(&args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).expect("JSON output must parse");
    assert_eq!(value["status"], "dry_run");
    assert_eq!(value["submitted"], false);
    let contracts = value["contracts"].as_array().expect("contracts array");
    assert_eq!(contracts.len(), 2);
    assert_eq!(contracts[0]["alias"], "token");
    assert_eq!(contracts[1]["alias"], "vault");
    for c in contracts {
        // derive_contract_id returns the 32-byte contract hash as hex (the
        // same form the single-deploy dry run prints and the record file
        // stores), not a C... strkey.
        let id = c["contractId"].as_str().unwrap();
        assert_eq!(id.len(), 64, "contractId is 32 bytes of hex");
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
        assert_eq!(c["wasmHash"], wasm_hash_hex());
        assert_eq!(c["salt"], SALT_HEX);
        assert_eq!(c["skipped"], false);
    }
    assert_eq!(mock.send_count(), 0, "dry run must not submit");
}

#[test]
fn dry_run_with_skip_deployed_marks_live_contract_skipped() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project(project_dir);
    generate_default_identity(project_dir);

    // Seed a record for token; the mock reports its instance as live.
    let mut file = DeploymentRecordFile::default();
    file.set_record(
        "testnet",
        "token",
        DeploymentRecord {
            contract_id: RECORDED_CONTRACT.to_string(),
            wasm_hash: wasm_hash_hex(),
            network: "testnet".to_string(),
            timestamp: 1_700_000_000,
            salt: Some(SALT_HEX.to_string()),
        },
    );
    file.write(project_dir.join(DEPLOYMENT_RECORD_FILE))
        .unwrap();

    let mock = MockServer::start(true);
    let mut args = deploy_base_args(&mock);
    args.push("--dry-run".into());
    args.push("--salt".into());
    args.push(SALT_HEX.into());
    args.push("--skip-deployed".into());
    args.push("--format".into());
    args.push("json".into());
    let output = sdkt(project_dir)
        .current_dir(project_dir)
        .args(&args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).expect("JSON output must parse");
    let contracts = value["contracts"].as_array().expect("contracts array");
    assert_eq!(contracts.len(), 2);
    // token is skipped with its RECORDED address, not a fresh prediction.
    assert_eq!(contracts[0]["alias"], "token");
    assert_eq!(contracts[0]["skipped"], true);
    assert_eq!(contracts[0]["contractId"], RECORDED_CONTRACT);
    // vault is still predicted.
    assert_eq!(contracts[1]["alias"], "vault");
    assert_eq!(contracts[1]["skipped"], false);
    assert_ne!(contracts[1]["contractId"], RECORDED_CONTRACT);
    assert_eq!(mock.send_count(), 0, "dry run must not submit");
}

#[test]
fn dry_run_does_not_need_the_secret_key() {
    // Regression test (CodeRabbit review on PR #569): the dry run must
    // resolve the source account from the stored public key without ever
    // loading the secret. With the secret_key line stripped from the
    // identity file, the dry run still succeeds.
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project(project_dir);
    generate_default_identity(project_dir);

    let identity_file = project_dir.join("identity").join("alice.toml");
    let content = std::fs::read_to_string(&identity_file).unwrap();
    assert!(
        content.contains("secret_key"),
        "fixture must have a secret to strip"
    );
    let stripped: String = content
        .lines()
        .filter(|l| !l.trim_start().starts_with("secret_key"))
        .map(|l| format!("{l}\n"))
        .collect();
    assert!(
        stripped.contains("public_key"),
        "stripped file must keep the public key"
    );
    std::fs::write(&identity_file, stripped).unwrap();

    let mock = MockServer::start(false);
    let mut args = deploy_base_args(&mock);
    args.push("--dry-run".into());
    args.push("--salt".into());
    args.push(SALT_HEX.into());
    sdkt(project_dir)
        .current_dir(project_dir)
        .args(&args)
        .assert()
        .success()
        .stdout(predicate::str::contains("No transactions submitted."));
    assert_eq!(mock.send_count(), 0, "dry run must not submit");
}

#[test]
fn dry_run_predictions_match_a_real_deploy_with_the_same_salt() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project(project_dir);
    generate_default_identity(project_dir);
    let mock = MockServer::start(false);

    // Real deploy against the mock with a fixed salt; the record file holds
    // the addresses the deploy path derived.
    let mut args = deploy_base_args(&mock);
    args.push("--salt".into());
    args.push(SALT_HEX.into());
    sdkt(project_dir)
        .current_dir(project_dir)
        .args(&args)
        .assert()
        .success();
    let record = DeploymentRecordFile::read(project_dir.join(DEPLOYMENT_RECORD_FILE)).unwrap();
    let token_record = record.record_for("testnet", "token").unwrap();
    let vault_record = record.record_for("testnet", "vault").unwrap();

    // Dry run with the same salt must predict exactly those addresses.
    let mut args = deploy_base_args(&mock);
    args.push("--dry-run".into());
    args.push("--salt".into());
    args.push(SALT_HEX.into());
    args.push("--format".into());
    args.push("json".into());
    let output = sdkt(project_dir)
        .current_dir(project_dir)
        .args(&args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).expect("JSON output must parse");
    let contracts = value["contracts"].as_array().expect("contracts array");
    assert_eq!(contracts[0]["contractId"], token_record.contract_id);
    assert_eq!(contracts[1]["contractId"], vault_record.contract_id);
    assert_eq!(contracts[0]["wasmHash"], token_record.wasm_hash);
    assert_eq!(contracts[1]["wasmHash"], vault_record.wasm_hash);
}
