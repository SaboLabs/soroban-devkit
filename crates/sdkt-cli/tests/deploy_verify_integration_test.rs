//! Post-deploy on-chain WASM hash verification (issue #89).
//!
//! `sdkt deploy` verifies the deployed contract against the artifact bytes
//! already in memory (reusing `verify_contract`, the same check `sdkt verify`
//! runs) and `sdkt project deploy` verifies every contract it deploys.
//! All network traffic is served by a mock JSON-RPC server — no live node,
//! no identity funds — and the mock answers `getLedgerEntries` by key kind:
//! account keys get the sequence entry, contract-instance keys get an
//! instance entry carrying the configured WASM hash, code keys get nothing
//! (the bytecode fetch inside `inspect_contract` is best-effort).
//!
//! - deploy success + matching on-chain hash → `Verified: ... ✓`, exit 0
//! - deploy success + mismatching hash → loud warning + exit 3, deploy
//!   output still printed (the deployment settled; only verification failed)
//! - contract not yet visible → warning + exit 3
//! - `--no-verify` → no verification RPC call, exit 0, `verification.skipped`
//! - project deploy verifies each deployed contract the same way

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use predicates::prelude::*;
use serde_json::Value;
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

/// LedgerEntry XDR for an account with seq_num 41 (so next sequence = 42).
const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";
/// SorobanTransactionData XDR: empty footprint, 1000 instructions, 150 stroops
/// resource fee.
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";
/// SHA-256 of the shipped `us_new.wasm` fixture (the on-chain hash the mock
/// serves for the MATCH case; taken from the fixture, never invented).
const US_NEW_HASH: &str = "551c5a9c7fd4c4a71e57e9c8a0ece1e7bd506ea065993c63db2dc35516406775";
/// A different 32-byte hash for the DRIFT case.
const OTHER_HASH: &str = "60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02";
const SALT_HEX: &str = "4242424242424242424242424242424242424242";
const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

fn us_new_path() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_new.wasm").to_string()
}

fn encode(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    let mut l = Limited::new(&mut cursor, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

/// Instance entry for any contract address, carrying `hash_hex` as its WASM
/// executable hash. The address is irrelevant to `inspect_contract` (it
/// decodes the executable out of the first entry), so one zero-address entry
/// serves every contract the deploy flow asks about.
fn instance_entry_xdr(hash_hex: &str) -> String {
    let mut hash = [0u8; 32];
    hex::decode_to_slice(hash_hex, &mut hash).unwrap();
    encode(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(hash)),
                storage: None,
            }),
        }),
        ext: LedgerEntryExt::V0,
    })
}

/// Read one complete HTTP request from `sock`: all headers plus the number of
/// body bytes declared in `Content-Length`.
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

/// Which on-chain WASM hash the mock serves for contract-instance lookups.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ServedHash {
    /// The fixture's own hash: verification matches.
    Match,
    /// A different hash: verification mismatches.
    Drift,
    /// No instance entry: the contract is not (yet) visible on-chain.
    NotFound,
}

/// Mock JSON-RPC server for the deploy + verify flow.
///
/// - `getLedgerEntries`: account keys → account entry (sequence lookup);
///   contract-instance keys → instance entry carrying the configured hash
///   (or no entries for `NotFound`); anything else → no entries.
/// - `simulateTransaction` → success; `sendTransaction` → PENDING;
///   `getTransaction` → SUCCESS.
/// - `verify_calls` counts `getLedgerEntries` requests carrying a
///   contract-instance key — i.e. exactly the verification round-trips —
///   so `--no-verify` can be proven to make none.
struct MockRpc {
    url: String,
    verify_calls: Arc<AtomicUsize>,
}

impl MockRpc {
    fn start(served: ServedHash) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let verify_calls = Arc::new(AtomicUsize::new(0));
        let verify_calls_thread = verify_calls.clone();
        let instance_match = instance_entry_xdr(US_NEW_HASH);
        let instance_drift = instance_entry_xdr(OTHER_HASH);

        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut sock) = conn else { break };
                let req = read_full_request(&mut sock);
                if req.is_empty() {
                    continue;
                }
                let method = extract_jsonrpc_method(&req);
                let body = match method.as_deref() {
                    Some("getLedgerEntries") => {
                        let keys = parse_get_ledger_keys(&req);
                        let has_account = keys.iter().any(|k| is_account_key(k));
                        let has_instance = keys.iter().any(|k| is_instance_key(k));
                        if has_instance {
                            verify_calls_thread.fetch_add(1, Ordering::SeqCst);
                        }
                        if has_account {
                            format!(
                                r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                            )
                        } else if has_instance {
                            match served {
                                ServedHash::Match => format!(
                                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{instance_match}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                                ),
                                ServedHash::Drift => format!(
                                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{instance_drift}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                                ),
                                ServedHash::NotFound => {
                                    r#"{"jsonrpc":"2.0","id":1,"result":{"entries":[],"latestLedger":100}}"#
                                        .to_string()
                                }
                            }
                        } else {
                            r#"{"jsonrpc":"2.0","id":1,"result":{"entries":[],"latestLedger":100}}"#
                                .to_string()
                        }
                    }
                    Some("simulateTransaction") => format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                    ),
                    Some("sendTransaction") => {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeef1234","status":"PENDING","latestLedger":"100"}}"#
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
                let _ = sock.flush();
            }
        });

        Self { url, verify_calls }
    }

    fn verify_call_count(&self) -> usize {
        self.verify_calls.load(Ordering::SeqCst)
    }
}

fn sdkt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

fn generate_identity(dir: &Path, name: &str) {
    sdkt(dir)
        .args(["identity", "generate", name])
        .assert()
        .success();
}

/// Run `sdkt deploy --wasm <fixture> ...` against `mock`, returning the
/// process output. `extra` holds trailing flags such as `--no-verify` or
/// `--format json`.
fn deploy_wasm(dir: &Path, mock: &MockRpc, extra: &[&str]) -> std::process::Output {
    let mut cmd = sdkt(dir);
    cmd.args([
        "deploy",
        "--wasm",
        &us_new_path(),
        "--salt",
        SALT_HEX,
        "--identity",
        "deploy-test",
        "--rpc-url",
        &mock.url,
        "--network-passphrase",
        TESTNET_PASSPHRASE,
    ]);
    cmd.args(extra);
    cmd.output().unwrap()
}

/// Run `sdkt deploy --wasm-hash <hash> ...` (create-only path) against `mock`.
fn deploy_wasm_hash(
    dir: &Path,
    mock: &MockRpc,
    hash: &str,
    extra: &[&str],
) -> std::process::Output {
    let mut cmd = sdkt(dir);
    cmd.args([
        "deploy",
        "--wasm-hash",
        hash,
        "--salt",
        SALT_HEX,
        "--identity",
        "deploy-test",
        "--rpc-url",
        &mock.url,
        "--network-passphrase",
        TESTNET_PASSPHRASE,
    ]);
    cmd.args(extra);
    cmd.output().unwrap()
}

fn stdout_json(output: &std::process::Output) -> Value {
    String::from_utf8_lossy(&output.stdout)
        .parse()
        .expect("stdout is valid JSON")
}

#[test]
fn deploy_matching_hash_prints_verified_and_exits_zero() {
    let dir = TempDir::new().unwrap();
    generate_identity(dir.path(), "deploy-test");
    let mock = MockRpc::start(ServedHash::Match);

    let output = deploy_wasm(dir.path(), &mock, &[]);

    assert!(output.status.success(), "matching hash must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Contract ID:"),
        "deploy output kept: {stdout}"
    );
    assert!(
        stdout.contains("Verified: on-chain WASM hash matches local artifact ✓"),
        "verified line printed: {stdout}"
    );
    assert_eq!(
        mock.verify_call_count(),
        1,
        "verification is exactly one instance lookup"
    );
}

#[test]
fn deploy_matching_hash_json_reports_verified() {
    let dir = TempDir::new().unwrap();
    generate_identity(dir.path(), "deploy-test");
    let mock = MockRpc::start(ServedHash::Match);

    let output = deploy_wasm(dir.path(), &mock, &["--format", "json"]);

    assert!(output.status.success(), "matching hash must exit 0");
    let json = stdout_json(&output);
    assert_eq!(json["verification"]["status"], "verified");
    assert_eq!(json["verification"]["onChainWasmHash"], US_NEW_HASH);
    assert_eq!(json["verification"]["localWasmHash"], US_NEW_HASH);
    // Existing deploy fields are untouched.
    assert_eq!(json["status"], "SUCCESS");
    assert!(!json["contractId"].as_str().unwrap_or_default().is_empty());
    assert_eq!(mock.verify_call_count(), 1);
}

#[test]
fn deploy_mismatched_hash_warns_and_exits_three() {
    let dir = TempDir::new().unwrap();
    generate_identity(dir.path(), "deploy-test");
    let mock = MockRpc::start(ServedHash::Drift);

    let output = deploy_wasm(dir.path(), &mock, &[]);

    assert_eq!(
        output.status.code(),
        Some(3),
        "mismatch must exit 3 (deploy settled, verification failed)"
    );
    // The deploy itself succeeded: its output is still printed.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Contract ID:"),
        "deploy output kept: {stdout}"
    );
    assert!(
        !stdout.contains("Verified: on-chain WASM hash matches local artifact"),
        "no verified line on mismatch: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Warning: post-deploy verification FAILED"),
        "loud warning on stderr: {stderr}"
    );
    assert!(
        stderr.contains(OTHER_HASH) && stderr.contains(US_NEW_HASH),
        "warning names both hashes: {stderr}"
    );
    assert_eq!(mock.verify_call_count(), 1);
}

#[test]
fn deploy_mismatched_hash_json_reports_failed() {
    let dir = TempDir::new().unwrap();
    generate_identity(dir.path(), "deploy-test");
    let mock = MockRpc::start(ServedHash::Drift);

    let output = deploy_wasm(dir.path(), &mock, &["--format", "json"]);

    assert_eq!(output.status.code(), Some(3));
    let json = stdout_json(&output);
    assert_eq!(json["verification"]["status"], "failed");
    assert_eq!(json["verification"]["onChainWasmHash"], OTHER_HASH);
    assert_eq!(json["verification"]["localWasmHash"], US_NEW_HASH);
    assert!(
        json["verification"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains(OTHER_HASH) && r.contains(US_NEW_HASH)),
        "reason names both hashes: {json}"
    );
    // The warning still goes to stderr so JSON stdout stays parseable.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Warning: post-deploy verification FAILED"));
}

#[test]
fn deploy_contract_not_yet_visible_warns_and_exits_three() {
    let dir = TempDir::new().unwrap();
    generate_identity(dir.path(), "deploy-test");
    let mock = MockRpc::start(ServedHash::NotFound);

    let output = deploy_wasm(dir.path(), &mock, &[]);

    assert_eq!(
        output.status.code(),
        Some(3),
        "unverifiable contract must exit 3, not 0 or 1"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Contract ID:"),
        "deploy output kept: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Warning: post-deploy verification FAILED"));
    assert!(
        stderr.contains("could not verify"),
        "warning explains the lookup failed: {stderr}"
    );
}

#[test]
fn deploy_no_verify_makes_no_verification_call() {
    let dir = TempDir::new().unwrap();
    generate_identity(dir.path(), "deploy-test");
    let mock = MockRpc::start(ServedHash::Match);

    let output = deploy_wasm(dir.path(), &mock, &["--no-verify"]);

    assert!(output.status.success(), "--no-verify must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Contract ID:"),
        "deploy output kept: {stdout}"
    );
    assert!(
        !stdout.contains("Verified:"),
        "no verification line with --no-verify: {stdout}"
    );
    assert_eq!(
        mock.verify_call_count(),
        0,
        "--no-verify must make no verification RPC call"
    );
}

#[test]
fn deploy_no_verify_json_reports_skipped() {
    let dir = TempDir::new().unwrap();
    generate_identity(dir.path(), "deploy-test");
    let mock = MockRpc::start(ServedHash::Match);

    let output = deploy_wasm(dir.path(), &mock, &["--no-verify", "--format", "json"]);

    assert!(output.status.success());
    let json = stdout_json(&output);
    assert_eq!(json["verification"]["status"], "skipped");
    assert_eq!(mock.verify_call_count(), 0);
}

#[test]
fn deploy_from_hash_verifies_against_flag_hash() {
    let dir = TempDir::new().unwrap();
    generate_identity(dir.path(), "deploy-test");
    let mock = MockRpc::start(ServedHash::Match);

    // Create-only path: the expected hash is the flag value itself.
    let output = deploy_wasm_hash(dir.path(), &mock, US_NEW_HASH, &[]);

    assert!(output.status.success(), "matching flag hash must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Verified: on-chain WASM hash matches local artifact ✓"),
        "verified line printed: {stdout}"
    );
    assert_eq!(mock.verify_call_count(), 1);
}

#[test]
fn deploy_from_hash_mismatch_exits_three() {
    let dir = TempDir::new().unwrap();
    generate_identity(dir.path(), "deploy-test");
    let mock = MockRpc::start(ServedHash::Drift);

    let output = deploy_wasm_hash(dir.path(), &mock, US_NEW_HASH, &[]);

    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Warning: post-deploy verification FAILED"));
}

// --- project deploy ---

/// Set up a single-contract project (same shape as
/// `project_deploy_record_test.rs`).
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
    std::fs::write(
        target.join("token.wasm"),
        std::fs::read(us_new_path()).unwrap(),
    )
    .unwrap();
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

/// Run `sdkt project deploy` in `dir` against `mock`. Network flags are
/// flattened on the `project` command, so they come before `deploy`.
fn project_deploy(dir: &Path, mock: &MockRpc, extra: &[&str]) -> std::process::Output {
    let mut cmd = sdkt(dir);
    cmd.current_dir(dir);
    cmd.args([
        "project",
        "--rpc-url",
        &mock.url,
        "--network-passphrase",
        TESTNET_PASSPHRASE,
        "deploy",
    ]);
    cmd.args(extra);
    cmd.output().unwrap()
}

#[test]
fn project_deploy_verifies_each_contract() {
    let dir = TempDir::new().unwrap();
    setup_single_contract_project(dir.path());
    generate_default_identity(dir.path());
    let mock = MockRpc::start(ServedHash::Match);

    let output = project_deploy(dir.path(), &mock, &[]);

    assert!(output.status.success(), "matching hash must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Contract ID:"),
        "deploy output kept: {stdout}"
    );
    assert!(
        stdout.contains("Verified: on-chain WASM hash matches local artifact ✓"),
        "verified line printed per contract: {stdout}"
    );
    assert_eq!(mock.verify_call_count(), 1);
}

#[test]
fn project_deploy_verification_json_reports_per_contract_status() {
    let dir = TempDir::new().unwrap();
    setup_single_contract_project(dir.path());
    generate_default_identity(dir.path());
    let mock = MockRpc::start(ServedHash::Match);

    let output = project_deploy(dir.path(), &mock, &["--format", "json"]);

    assert!(output.status.success());
    let json = stdout_json(&output);
    assert_eq!(json["status"], "success");
    assert_eq!(json["verification"]["token"]["status"], "verified");
    assert_eq!(
        json["verification"]["token"]["onChainWasmHash"],
        US_NEW_HASH
    );
    assert!(!json["contracts_deployed"]["token"]
        .as_str()
        .unwrap_or_default()
        .is_empty());
    assert_eq!(mock.verify_call_count(), 1);
}

#[test]
fn project_deploy_mismatch_warns_and_exits_three() {
    let dir = TempDir::new().unwrap();
    setup_single_contract_project(dir.path());
    generate_default_identity(dir.path());
    let mock = MockRpc::start(ServedHash::Drift);

    let output = project_deploy(dir.path(), &mock, &[]);

    assert_eq!(
        output.status.code(),
        Some(3),
        "mismatch must exit 3 even though every deploy settled"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Contract ID:"),
        "deploy output kept: {stdout}"
    );
    assert!(stdout.contains("Project deployment complete."));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Warning: post-deploy verification FAILED"));
}

#[test]
fn project_deploy_no_verify_makes_no_verification_call() {
    let dir = TempDir::new().unwrap();
    setup_single_contract_project(dir.path());
    generate_default_identity(dir.path());
    let mock = MockRpc::start(ServedHash::Match);

    let output = project_deploy(dir.path(), &mock, &["--no-verify"]);

    assert!(output.status.success(), "--no-verify must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Verified:"),
        "no verification line with --no-verify: {stdout}"
    );
    assert_eq!(mock.verify_call_count(), 0);
}

#[test]
fn deploy_help_lists_no_verify() {
    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--no-verify"));
}
