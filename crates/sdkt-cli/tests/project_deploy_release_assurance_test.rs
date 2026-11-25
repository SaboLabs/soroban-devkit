//! Integration tests for `sdkt project deploy --release-assurance` (#545).
//!
//! Verifies:
//! 1. Preflight FAIL on the second contract aborts with exit 1 before any upload is attempted.
//! 2. Preflight PASS on all contracts continues to the normal deploy flow (with Upgrade Safety Skipped when no record exists).
//! 3. A contract skipped via `--skip-deployed` is excluded from preflight (its audit findings do not block the deploy).
//! 4. Without `--release-assurance`, the same workspace with a failing audit source deploys unchanged.

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use predicates::prelude::*;
use sdkt_core::deployment::{DeploymentRecord, DeploymentRecordFile, DEPLOYMENT_RECORD_FILE};
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

static CONTRACT_WASM: &[u8] = include_bytes!("fixtures/us_new.wasm");
const US_NEW_HASH: &str = "551c5a9c7fd4c4a71e57e9c8a0ece1e7bd506ea065993c63db2dc35516406775";
const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";
const RECORDED_CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

fn sdkt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

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

/// Set up a two-contract workspace (`token` -> `vault`).
/// `token` has clean Rust source; `vault` can be configured with clean or
/// critical-finding Rust source (unguarded `#[contractimpl] pub fn set_admin`
/// state write triggering AUTH-001 critical).
fn setup_two_contract_workspace(dir: &Path, vault_has_critical_audit_finding: bool) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"

[contracts.vault]
path = "contracts/vault"
depends_on = ["token"]
"#;
    std::fs::write(dir.join(".sdkt.toml"), manifest).unwrap();

    for alias in ["token", "vault"] {
        let contract_dir = dir.join("contracts").join(alias);
        let src_dir = contract_dir.join("src");
        let target = contract_dir
            .join("target")
            .join(sdkt_core::build::WASM_BUILD_TARGET)
            .join("release");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join(format!("{alias}.wasm")), CONTRACT_WASM).unwrap();

        let rs_source = if alias == "vault" && vault_has_critical_audit_finding {
            r#"
#[contractimpl]
impl Vault {
    pub fn set_admin(env: Env, new_admin: Address) {
        env.storage().instance().set(&Symbol::new(&env, "admin"), &new_admin);
    }
}
"#
        } else {
            r#"
#[contractimpl]
impl CleanContract {
    pub fn get_value(env: Env) -> u32 {
        42
    }
}
"#
        };
        std::fs::write(src_dir.join("lib.rs"), rs_source).unwrap();
    }
}

fn contract_instance_entry_xdr() -> String {
    let mut hash = [0u8; 32];
    hex::decode_to_slice(US_NEW_HASH, &mut hash).unwrap();
    let entry = LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(hash)),
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

fn is_instance_key(key: &str) -> bool {
    matches!(
        sdkt_xdr::decode_ledger_key(key),
        Ok(stellar_xdr::LedgerKey::ContractData(_))
    )
}

struct MockDeployServer {
    url: String,
    sim_count: Arc<AtomicUsize>,
    send_count: Arc<AtomicUsize>,
}

impl MockDeployServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let sim_count = Arc::new(AtomicUsize::new(0));
        let send_count = Arc::new(AtomicUsize::new(0));
        let sim_clone = Arc::clone(&sim_count);
        let send_clone = Arc::clone(&send_count);
        let instance_xdr = contract_instance_entry_xdr();

        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut sock = stream;
                let req = read_full_request(&mut sock);
                let method = extract_jsonrpc_method(&req).unwrap_or_default();

                let body = match method.as_str() {
                    "getLedgerEntries" => {
                        let keys = parse_get_ledger_keys(&req);
                        let mut entries = Vec::new();
                        for key in keys {
                            if is_account_key(&key) {
                                entries.push(serde_json::json!({
                                    "key": key,
                                    "xdr": ACCOUNT_ENTRY_XDR,
                                    "lastModifiedLedgerSeq": 100
                                }));
                            } else if is_instance_key(&key) {
                                entries.push(serde_json::json!({
                                    "key": key,
                                    "xdr": instance_xdr,
                                    "lastModifiedLedgerSeq": 100
                                }));
                            }
                        }
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "result": { "entries": entries, "latestLedger": 100 }
                        })
                        .to_string()
                    }
                    "simulateTransaction" => {
                        sim_clone.fetch_add(1, Ordering::SeqCst);
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "result": {
                                "transactionData": SOROBAN_DATA_XDR,
                                "minResourceFee": "150",
                                "results": [{ "xdr": "AAAAAwAAACo=", "auth": [] }],
                                "latestLedger": 100
                            }
                        })
                        .to_string()
                    }
                    "sendTransaction" => {
                        send_clone.fetch_add(1, Ordering::SeqCst);
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "result": {
                                "status": "PENDING",
                                "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                                "latestLedger": 100,
                                "latestLedgerCloseTime": "1700000000"
                            }
                        })
                        .to_string()
                    }
                    "getTransaction" => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "status": "SUCCESS",
                            "ledger": 101,
                            "createdAt": "1700000001",
                            "applicationOrder": 1,
                            "feeBump": false,
                            "envelopeXdr": "",
                            "resultXdr": "",
                            "resultMetaXdr": ""
                        }
                    })
                    .to_string(),
                    _ => serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "method not found"}}).to_string(),
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
}

#[test]
fn project_deploy_help_documents_release_assurance() {
    let dir = TempDir::new().unwrap();
    sdkt(dir.path())
        .args(["project", "deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--release-assurance"));
}

#[test]
fn project_deploy_release_assurance_aborts_before_any_upload_when_second_contract_fails_audit() {
    let dir = TempDir::new().unwrap();
    generate_default_identity(dir.path());
    setup_two_contract_workspace(dir.path(), true);
    let server = MockDeployServer::start();

    sdkt(dir.path())
        .current_dir(dir.path())
        .args([
            "project",
            "deploy",
            "--release-assurance",
            "--rpc-url",
            &server.url,
            "--network-passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "release-assurance preflight failed for 'vault'",
        ));

    assert_eq!(
        server.sim_count.load(Ordering::SeqCst),
        0,
        "no upload or simulateTransaction may be attempted when preflight fails"
    );
    assert_eq!(
        server.send_count.load(Ordering::SeqCst),
        0,
        "no sendTransaction may be attempted when preflight fails"
    );
}

#[test]
fn project_deploy_without_release_assurance_flag_deploys_unchanged() {
    let dir = TempDir::new().unwrap();
    generate_default_identity(dir.path());
    setup_two_contract_workspace(dir.path(), true);
    let server = MockDeployServer::start();

    sdkt(dir.path())
        .current_dir(dir.path())
        .args([
            "project",
            "deploy",
            "--rpc-url",
            &server.url,
            "--network-passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project deployment complete"));

    assert!(
        server.send_count.load(Ordering::SeqCst) > 0,
        "normal deploy without --release-assurance proceeds with uploads"
    );
}

#[test]
fn project_deploy_release_assurance_passes_and_deploys_when_all_contracts_pass() {
    let dir = TempDir::new().unwrap();
    generate_default_identity(dir.path());
    setup_two_contract_workspace(dir.path(), false);
    let server = MockDeployServer::start();

    sdkt(dir.path())
        .current_dir(dir.path())
        .args([
            "project",
            "deploy",
            "--release-assurance",
            "--rpc-url",
            &server.url,
            "--network-passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Preflight 'token':"))
        .stdout(predicate::str::contains("Preflight 'vault':"))
        .stdout(predicate::str::contains("(Artifact: PASS, Security: PASS, Upgrade Safety: SKIPPED)"))
        .stdout(predicate::str::contains("Project deployment complete"));
}

#[test]
fn project_deploy_release_assurance_skips_preflight_for_skip_deployed_contracts() {
    let dir = TempDir::new().unwrap();
    generate_default_identity(dir.path());
    // vault has a critical audit finding, but it is already recorded as deployed
    // on-chain and --skip-deployed is passed, so its audit state must not block token's deploy.
    setup_two_contract_workspace(dir.path(), true);
    let server = MockDeployServer::start();

    let mut rec_file = DeploymentRecordFile::default();
    rec_file.set_record(
        "testnet",
        "vault",
        DeploymentRecord {
            contract_id: RECORDED_CONTRACT_ID.to_string(),
            wasm_hash: US_NEW_HASH.to_string(),
            network: "testnet".to_string(),
            timestamp: 1700000000,
            salt: None,
        },
    );
    rec_file
        .write(dir.path().join(DEPLOYMENT_RECORD_FILE))
        .unwrap();

    sdkt(dir.path())
        .current_dir(dir.path())
        .args([
            "project",
            "deploy",
            "--skip-deployed",
            "--release-assurance",
            "--rpc-url",
            &server.url,
            "--network-passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Preflight 'token':"))
        .stdout(predicate::str::contains("(Artifact: PASS, Security: PASS, Upgrade Safety: SKIPPED)"))
        .stdout(predicate::str::contains("Project deployment complete"));
}
