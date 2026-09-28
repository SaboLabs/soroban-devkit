use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

fn fixture(name: &str) -> String {
    let dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/tests/fixtures/{}", dir, name)
}

#[test]
fn diff_help_documents_upgrade_safety() {
    sdkt()
        .args(["diff", "--help"])
        .assert()
        .success()
        .stdout(contains("upgrade-safety"));
}

#[test]
fn upgrade_safety_pretty_shows_breaking_and_nonbreaking() {
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
        ])
        .assert()
        .success()
        .stdout(contains("Upgrade Safety"))
        .stdout(contains("Compatible: NO"))
        .stdout(contains("Removed function: mint"))
        .stdout(contains("Removed function: transfer"))
        .stdout(contains("Added function: hello"));
}

#[test]
fn upgrade_safety_json_serializes_verdict() {
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(contains("\"compatible\":false"))
        .stdout(contains("\"breaking_changes\""))
        .stdout(contains("\"non_breaking_changes\""));
}

#[test]
fn deploy_deny_breaking_aborts_on_incompatible() {
    sdkt()
        .args([
            "deploy",
            "--wasm",
            &fixture("us_new.wasm"),
            "--salt",
            "0000000000000000000000000000000000000000",
            "--deny-breaking",
            "--old-wasm",
            &fixture("us_old.wasm"),
        ])
        .assert()
        .failure()
        .stderr(contains("NOT backwards-compatible"));
}

#[test]
fn deploy_fails_without_identity() {
    // Deploy without a configured identity should fail with identity error,
    // NOT with upgrade-safety guard error.
    sdkt()
        .args([
            "deploy",
            "--wasm",
            &fixture("us_new.wasm"),
            "--salt",
            "0000000000000000000000000000000000000001",
        ])
        .assert()
        .failure() // Expected to fail due to missing identity
        .stderr(contains("NOT backwards-compatible").not());
}

// ── diff --upgrade-safety --deny-breaking exit-code tests ────────────────────

/// `diff --upgrade-safety` without `--deny-breaking` exits 0 even when the
/// verdict is breaking (existing / flagless behaviour is unchanged).
#[test]
fn diff_upgrade_safety_no_flag_exits_zero_on_breaking() {
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
        ])
        .assert()
        .success() // exit 0 — no --deny-breaking flag
        .stdout(contains("Compatible: NO"));
}

/// `diff --upgrade-safety --deny-breaking` exits non-zero (2) when breaking
/// changes are present, and the full report is still printed before exit.
#[test]
fn diff_deny_breaking_exits_nonzero_on_incompatible() {
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
            "--deny-breaking",
        ])
        .assert()
        .code(2)
        // Full report is still printed before exit.
        .stdout(contains("Upgrade Safety"))
        .stdout(contains("Compatible: NO"));
}

/// `diff --upgrade-safety --deny-breaking` exits 0 when the verdict is
/// compatible (comparing a WASM against itself — identical = no breaking changes).
#[test]
fn diff_deny_breaking_exits_zero_on_compatible() {
    // us_old.wasm vs itself: no changes at all → compatible verdict.
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_old.wasm"),
            "--upgrade-safety",
            "--deny-breaking",
        ])
        .assert()
        .success() // exit 0 — compatible
        .stdout(contains("Compatible: YES"));
}

// ── diff --help documents --deny-breaking ───────────────────────────────────

/// `sdkt diff --help` must advertise the new `--deny-breaking` flag so users
/// can discover it.
#[test]
fn diff_help_documents_deny_breaking() {
    sdkt()
        .args(["diff", "--help"])
        .assert()
        .success()
        .stdout(contains("deny-breaking"));
}

// ── verify --help documents --deny-breaking ─────────────────────────────────

/// `sdkt verify --help` must advertise `--deny-breaking`.
#[test]
fn verify_help_documents_deny_breaking() {
    sdkt()
        .args(["verify", "--help"])
        .assert()
        .success()
        .stdout(contains("deny-breaking"));
}

// ── Mock RPC integration tests for verify --upgrade-safety gating ────────────

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use stellar_xdr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ContractId, ExtensionPoint, Hash, LedgerEntry, LedgerEntryData,
    LedgerEntryExt, Limited, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};

static VERIFY_OLD_WASM: &[u8] = include_bytes!("fixtures/us_old.wasm");
const VERIFY_CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const VERIFY_OLD_HASH_HEX: &str =
    "05befa136e7f0829a5051d97b032f355a5e65976397df90b224d141942dce46c";

fn spawn_mock_verify_rpc() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let contract_id_hex = "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c";
    let contract_data_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(contract_id_hex.to_string())).unwrap();
    let contract_code_key = encode_ledger_key(&LedgerKeyParams::ContractCode(
        VERIFY_OLD_HASH_HEX.to_string(),
    ))
    .unwrap();

    let mut wasm_hash = [0u8; 32];
    hex::decode_to_slice(VERIFY_OLD_HASH_HEX, &mut wasm_hash).unwrap();

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
            code: VERIFY_OLD_WASM.to_vec().try_into().unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    });

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

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

/// `verify --upgrade-safety --deny-breaking` exits code 2 when breaking
/// changes are detected between on-chain WASM and candidate WASM, and full report
/// is printed before exit.
#[test]
fn verify_upgrade_safety_deny_breaking_exits_nonzero_on_incompatible() {
    let mock_url = spawn_mock_verify_rpc();
    sdkt()
        .args([
            "verify",
            "--contract",
            VERIFY_CONTRACT_ID,
            "--wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
            "--deny-breaking",
            "--rpc-url",
            &mock_url,
        ])
        .assert()
        .code(2)
        .stdout(contains("Upgrade Safety"))
        .stdout(contains("Compatible: NO"));
}

/// `verify --upgrade-safety` without `--deny-breaking` exits 0 even when breaking
/// changes are present.
#[test]
fn verify_upgrade_safety_no_flag_exits_zero_on_breaking() {
    let mock_url = spawn_mock_verify_rpc();
    sdkt()
        .args([
            "verify",
            "--contract",
            VERIFY_CONTRACT_ID,
            "--wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
            "--rpc-url",
            &mock_url,
        ])
        .assert()
        .success()
        .stdout(contains("Upgrade Safety"))
        .stdout(contains("Compatible: NO"));
}

/// `verify --upgrade-safety --deny-breaking` exits 0 when changes are compatible.
#[test]
fn verify_upgrade_safety_deny_breaking_exits_zero_on_compatible() {
    let mock_url = spawn_mock_verify_rpc();
    sdkt()
        .args([
            "verify",
            "--contract",
            VERIFY_CONTRACT_ID,
            "--wasm",
            &fixture("us_old.wasm"),
            "--upgrade-safety",
            "--deny-breaking",
            "--rpc-url",
            &mock_url,
        ])
        .assert()
        .success()
        .stdout(contains("Upgrade Safety"))
        .stdout(contains("Compatible: YES"));
}

// ── health --help documents --fail-on ───────────────────────────────────────

/// `sdkt health --help` must advertise the new `--fail-on` flag.
#[test]
fn health_help_documents_fail_on() {
    sdkt()
        .args(["health", "--help"])
        .assert()
        .success()
        .stdout(contains("fail-on"));
}
