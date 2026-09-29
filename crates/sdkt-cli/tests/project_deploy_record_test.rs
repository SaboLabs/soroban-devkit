//! Integration tests for `sdkt project deploy` deployment records and the
//! `--skip-deployed` resume path.
//!
//! Every network interaction is served by an in-process mock JSON-RPC server
//! (same pattern as `invoke_integration_test.rs`), so no live Testnet is
//! required. The tests exercise:
//!
//! - a failed deploy persists the records of the contracts that deployed before
//!   the failure (never losing on-chain addresses),
//! - `--skip-deployed` resumes an interrupted deploy, skipping only aliases
//!   whose recorded contract still exists on-chain,
//! - `--skip-deployed` does NOT skip when the recorded contract is gone
//!   (the record file alone is not trusted),
//! - the success-path JSON output is unchanged by the record file.

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
    Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limited, Limits, ReadXdr, ScAddress,
    ScContractInstance, ScVal, TransactionEnvelope, WriteXdr,
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

/// A synthetic contract ID that exists only in the seeded record file.
const STALE_RECORDED_CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

fn sdkt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

/// Set up a 3-contract project (token ← vault ← router) with real WASM
/// artifacts under each contract's release target directory, so
/// `resolve_project` resolves a valid deploy order without running `sdkt build`.
fn setup_project(dir: &Path) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"

[contracts.vault]
path = "contracts/vault"
depends_on = ["token"]

[contracts.router]
path = "contracts/router"
depends_on = ["vault"]
"#;
    std::fs::write(dir.join(".sdkt.toml"), manifest).unwrap();

    for alias in ["token", "vault", "router"] {
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

/// Set up a single-contract project.
fn setup_single_contract_project(dir: &Path) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"
"#;
    std::fs::write(dir.join(".sdkt.toml"), manifest).unwrap();
    let target = dir
        .join("contracts")
        .join("token")
        .join("target")
        .join(sdkt_core::build::WASM_BUILD_TARGET)
        .join("release");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("token.wasm"), CONTRACT_WASM).unwrap();
}

/// Create a usable signing identity for `project deploy`, which resolves the
/// hardcoded `default` identity via the `sdkt identity default` command (which
/// creates the `default` symlink that `get_default` reads).
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

/// Read one complete HTTP request from `sock`: all headers plus the number of
/// body bytes declared in `Content-Length`. Handles requests whose body arrives
/// in multiple `read()` calls (e.g. large WASM upload envelopes).
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

/// Mock JSON-RPC server for the `project deploy` flow.
///
/// - `getLedgerEntries`: the account key always returns an account entry; the
///   contract instance key returns a live contract entry when `instance_live`
///   is true, empty entries otherwise (record-file-only resume).
/// - `simulateTransaction`: succeeds until the `fail_on_sim`-th call
///   (1-indexed across all deployments), which returns a simulation error so
///   the current contract's deploy fails.
/// - `sendTransaction` → PENDING; `getTransaction` → SUCCESS.
struct MockServer {
    url: String,
    sim_count: Arc<AtomicUsize>,
}

impl MockServer {
    fn start(instance_live: bool, fail_on_sim: Option<usize>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let sim_count = Arc::new(AtomicUsize::new(0));
        let sim_count_thread = sim_count.clone();
        let instance_xdr = contract_instance_entry_xdr();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let mut sock = match conn {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let mut buf = [0u8; 16384];
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    continue;
                }
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
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
                        let n = sim_count_thread.fetch_add(1, Ordering::SeqCst) + 1;
                        if fail_on_sim == Some(n) {
                            r#"{"jsonrpc":"2.0","id":1,"result":{"error":"transaction polling timed out"}}"#
                                .to_string()
                        } else {
                            format!(
                                r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                            )
                        }
                    }
                    Some("sendTransaction") => {
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

        Self { url, sim_count }
    }

    fn simulate_count(&self) -> usize {
        self.sim_count.load(Ordering::SeqCst)
    }
}

/// Base invocation: `sdkt project <net args> deploy`. The network flags are
/// flattened on the `project` command (see `Commands::Project`), so they come
/// before the `deploy` subcommand.
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

fn read_record(dir: &Path) -> DeploymentRecordFile {
    DeploymentRecordFile::read(dir.join(DEPLOYMENT_RECORD_FILE)).expect("record file readable")
}

/// A deploy that fails on the second contract must persist the first contract's
/// record, and a `--skip-deployed` re-run must resume from the third.
#[test]
fn failed_project_deploy_persists_records_and_skip_deployed_resumes() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project(project_dir);
    generate_default_identity(project_dir);

    // Run 1: 3-contract graph where the second contract (vault) fails at the
    // 3rd simulation. token deploys; vault fails; router never attempted.
    let fail_mock = MockServer::start(false, Some(3));
    sdkt(project_dir)
        .current_dir(project_dir)
        .args(deploy_base_args(&fail_mock))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Deployment failed"))
        .stderr(predicate::str::contains(
            "Deployment record written to .sdkt-deployments.json (1 of 3 deployed)",
        ));

    // Record contains only token, with a real contract ID.
    let record = read_record(project_dir);
    let token = record
        .record_for("testnet", "token")
        .expect("token record persisted");
    assert!(!token.contract_id.is_empty());
    assert_eq!(token.network, "testnet");
    assert!(record.record_for("testnet", "vault").is_none());
    assert!(record.record_for("testnet", "router").is_none());

    // Run 2: --skip-deployed. token is recorded AND served as live on-chain,
    // so it is skipped; vault and router deploy fresh (2 simulations each).
    let resume_mock = MockServer::start(true, None);
    sdkt(project_dir)
        .current_dir(project_dir)
        .args({
            let mut args = deploy_base_args(&resume_mock);
            args.push("--skip-deployed".into());
            args
        })
        .assert()
        .success()
        .stdout(predicate::str::contains("already deployed at"))
        .stdout(predicate::str::contains("Deploying alias 'vault'"))
        .stdout(predicate::str::contains("Deploying alias 'router'"))
        .stdout(predicate::str::contains("Deploying alias 'token'").not());

    assert_eq!(
        resume_mock.simulate_count(),
        4,
        "resume must deploy only vault + router (2 simulations each)"
    );

    // Record now covers all three contracts for the "testnet" scope.
    let record = read_record(project_dir);
    assert!(
        record.record_for("testnet", "token").is_some(),
        "token record retained across the resume"
    );
    assert!(
        record.record_for("testnet", "vault").is_some(),
        "vault record persisted after resume"
    );
    assert!(
        record.record_for("testnet", "router").is_some(),
        "router record persisted after resume"
    );
}

/// `--skip-deployed` must NOT skip an alias whose recorded contract no longer
/// exists on-chain: the file entry alone is not trusted, the ledger is.
#[test]
fn skip_deployed_does_not_skip_gone_contract() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project(project_dir);
    generate_default_identity(project_dir);

    // Seed a record file claiming token was deployed at a contract that the
    // mock will report as absent (empty getLedgerEntries for instance keys).
    let mut seeded = DeploymentRecordFile::default();
    seeded.set_record(
        "testnet",
        "token",
        DeploymentRecord {
            contract_id: STALE_RECORDED_CONTRACT.into(),
            wasm_hash: "60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02".into(),
            network: "testnet".into(),
            timestamp: 1_700_000_000,
            salt: Some("deploy".into()),
        },
    );
    seeded
        .write(project_dir.join(DEPLOYMENT_RECORD_FILE))
        .unwrap();

    let mock = MockServer::start(false, None);
    sdkt(project_dir)
        .current_dir(project_dir)
        .args({
            let mut args = deploy_base_args(&mock);
            args.push("--skip-deployed".into());
            args
        })
        .assert()
        .success()
        .stderr(predicate::str::contains("no longer on-chain"))
        .stdout(predicate::str::contains("Deploying alias 'token'"))
        .stdout(predicate::str::contains("Deploying alias 'vault'"))
        .stdout(predicate::str::contains("Deploying alias 'router'"));

    assert_eq!(
        mock.simulate_count(),
        6,
        "a stale record must not skip: all 3 contracts deploy (2 simulations each)"
    );

    // The stale record was replaced with the fresh deployment's contract ID.
    let record = read_record(project_dir);
    let token = record
        .record_for("testnet", "token")
        .expect("token record overwritten after re-deploy");
    assert_ne!(token.contract_id, STALE_RECORDED_CONTRACT);
}

/// Regression: a fully successful deploy still emits exactly the same
/// success-path JSON document (`status` + `contracts_deployed`), with the
/// record file being purely additive.
#[test]
fn successful_deploy_json_output_is_unchanged() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_single_contract_project(project_dir);
    generate_default_identity(project_dir);

    let mock = MockServer::start(false, None);

    // Capture stdout as JSON and assert its shape is unchanged by the record file.
    let mut cmd = sdkt(project_dir);
    cmd.current_dir(project_dir);
    let mut args = deploy_base_args(&mock);
    args.push("--format".into());
    args.push("json".into());
    let output = cmd.args(&args).output().expect("runs");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["status"], "success");
    let deployed = parsed["contracts_deployed"]
        .as_object()
        .expect("contracts_deployed is an object");
    assert!(
        deployed.contains_key("token"),
        "json output lists the deployed token alias"
    );
    let cid = deployed["token"]
        .as_str()
        .expect("token contract id is a string");
    assert!(!cid.is_empty());

    // Record file is additive and matches the JSON output's contract ID.
    let record = read_record(project_dir);
    assert_eq!(
        record.record_for("testnet", "token").unwrap().contract_id,
        cid
    );
}

/// `--identity <name>` selects a non-default identity for the deployment.
/// Generates an identity named "deployer" (not "default") and passes it via
/// `--identity`. Asserts that the mock RPC receives the deployer's account key
/// in `getLedgerEntries`, proving the correct signing account was used.
#[test]
fn project_deploy_uses_explicit_identity() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_single_contract_project(project_dir);

    // Generate a named identity — do NOT set it as default.
    sdkt(project_dir)
        .args(["identity", "generate", "deployer"])
        .assert()
        .success();

    // Capture the deployer's public key (G-address) via `identity show`.
    let show_out = sdkt(project_dir)
        .args(["identity", "show", "deployer"])
        .output()
        .expect("identity show succeeds");
    let show_str = String::from_utf8_lossy(&show_out.stdout);
    let deployer_pubkey = show_str
        .lines()
        .find_map(|l| {
            let l = l.trim();
            if l.starts_with('G') && l.len() >= 56 {
                // Take the first G-address-looking token on any line
                Some(
                    l.split_whitespace()
                        .find(|t| t.starts_with('G'))?
                        .to_string(),
                )
            } else {
                // Also try extracting from a "key: GXXX" line
                let after_colon = l.split_once(':')?.1.trim();
                if after_colon.starts_with('G') && after_colon.len() >= 56 {
                    Some(after_colon.to_string())
                } else {
                    None
                }
            }
        })
        .expect("identity show must print the G-address");

    // Start a mock that records every account key queried via getLedgerEntries
    // AND captures each sendTransaction envelope for post-deploy signature
    // verification.
    let queried_accounts: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let queried_accounts_srv = queried_accounts.clone();

    let submitted_envelopes: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let submitted_envelopes_srv = submitted_envelopes.clone();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let mock_url = format!("http://{addr}");

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let req = read_full_request(&mut sock);
            let method = extract_jsonrpc_method(&req);

            if method.as_deref() == Some("getLedgerEntries") {
                let keys = parse_get_ledger_keys(&req);
                let mut acc = queried_accounts_srv.lock().unwrap();
                for k in keys {
                    if is_account_key(&k) {
                        acc.push(k);
                    }
                }
            }

            if method.as_deref() == Some("sendTransaction") {
                // Extract the body (everything after the blank line separating headers)
                if let Some(body_start) = req.find("\r\n\r\n") {
                    let body = &req[body_start + 4..];
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) {
                        if let Some(xdr_b64) = parsed["params"]["transaction"].as_str() {
                            submitted_envelopes_srv
                                .lock()
                                .unwrap()
                                .push(xdr_b64.to_string());
                        }
                    }
                }
            }

            let body = match method.as_deref() {
                Some("getLedgerEntries") => {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                    )
                }
                Some("simulateTransaction") => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                ),
                Some("sendTransaction") => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeefcafe","status":"PENDING","latestLedger":"100"}}"#
                        .to_string()
                }
                Some("getTransaction") => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg=="}}"#
                        .to_string()
                }
                _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string(),
            };

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    sdkt(project_dir)
        .current_dir(project_dir)
        .args(vec![
            "project".to_string(),
            "--rpc-url".to_string(),
            mock_url.clone(),
            "--network-passphrase".to_string(),
            "Test SDF Network ; September 2015".to_string(),
            "deploy".to_string(),
            "--identity".to_string(),
            "deployer".to_string(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Deploying alias 'token'"));

    // --- Assertion 1: the deployer's account key appeared in getLedgerEntries ---
    let keys_seen = queried_accounts.lock().unwrap().clone();
    assert!(
        !keys_seen.is_empty(),
        "expected at least one account key lookup"
    );
    let deployer_seen = keys_seen.iter().any(|k| {
        if let Ok(stellar_xdr::LedgerKey::Account(acct)) = sdkt_xdr::decode_ledger_key(k) {
            let stellar_xdr::PublicKey::PublicKeyTypeEd25519(bytes) = acct.account_id.0;
            let g = stellar_strkey::Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(
                bytes.0,
            ))
            .to_string()
            .as_str()
            .to_string();
            return g == deployer_pubkey;
        }
        false
    });
    assert!(
        deployer_seen,
        "expected the deployer's account key ({deployer_pubkey}) in getLedgerEntries, got: {keys_seen:?}"
    );

    // --- Assertion 2: every submitted envelope was signed by the deployer's key ---
    //
    // Read the deployer's secret key directly from its identity file, construct
    // an Ed25519Signer, and use sdkt_xdr::verify_signature to confirm that the
    // deployer's key produced at least one DecoratedSignature in each submitted
    // envelope.  This proves *which* key signed, not just which account was
    // looked up.
    let identity_file = project_dir.join("identity").join("deployer.toml");
    let identity_toml = std::fs::read_to_string(&identity_file).expect("deployer.toml must exist");
    let secret_key_str = identity_toml
        .lines()
        .find_map(|l| {
            let l = l.trim();
            if l.starts_with("secret_key") {
                let parts: Vec<&str> = l.splitn(2, '=').collect();
                if parts.len() == 2 {
                    return Some(parts[1].trim().trim_matches('"').to_string());
                }
            }
            None
        })
        .expect("secret_key field in deployer.toml");

    let deployer_signer = sdkt_xdr::Ed25519Signer::from_secret_str(&secret_key_str)
        .expect("deployer secret key must be valid");
    let signing_opts = sdkt_xdr::SigningOptions::with(sdkt_xdr::Network::Testnet);

    let envelopes = submitted_envelopes.lock().unwrap().clone();
    assert!(
        !envelopes.is_empty(),
        "expected at least one sendTransaction call"
    );
    for (i, xdr_b64) in envelopes.iter().enumerate() {
        let raw = STANDARD
            .decode(xdr_b64)
            .unwrap_or_else(|e| panic!("envelope {i} is not valid base64: {e}"));
        let mut cursor = std::io::Cursor::new(&raw);
        let mut limited = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let envelope = TransactionEnvelope::read_xdr(&mut limited)
            .unwrap_or_else(|e| panic!("envelope {i} XDR parse failed: {e}"));
        assert!(
            sdkt_xdr::verify_signature(&envelope, &deployer_signer, &signing_opts),
            "envelope {i} was not signed by the deployer's key ({deployer_pubkey})"
        );
    }
}

/// Passing a nonexistent identity name must fail immediately with the supplied
/// name present in the error message (not a hardcoded "Default identity").
#[test]
fn project_deploy_nonexistent_identity_error_contains_name() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_single_contract_project(project_dir);

    // No identity is generated at all; the name "no_such_identity" does not exist.
    let mock = MockServer::start(false, None);
    sdkt(project_dir)
        .current_dir(project_dir)
        .args({
            let mut args = deploy_base_args(&mock);
            args.push("--identity".into());
            args.push("no_such_identity".into());
            args
        })
        .assert()
        .failure()
        .stderr(predicate::str::contains("no_such_identity"));
}

/// `--help` for `project deploy` must document the `--identity` / `-I` option.
#[test]
fn project_deploy_help_shows_identity_option() {
    let project = TempDir::new().unwrap();
    sdkt(project.path())
        .args(["project", "deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--identity"))
        .stdout(predicate::str::contains("-I"));
}
