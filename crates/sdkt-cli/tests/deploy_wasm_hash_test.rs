//! Integration tests for create-only deploy via `--wasm-hash` (issue #70).
//!
//! - RPC: `deploy_contract_from_hash` drives a create-only flow (one
//!   simulate + one sendTransaction) and never runs the upload transaction.
//! - Contract-ID consistency: same salt + same wasm hash yields the same
//!   derived contract ID as the full `--wasm` deploy path.
//! - CLI: `--wasm` and `--wasm-hash` are mutually exclusive, and at least one
//!   is required (both offline, before any network I/O).

use assert_cmd::Command;
use predicates::prelude::*;
use sdkt_rpc::client::SorobanRpcClient;
use sdkt_rpc::{deploy_contract, deploy_contract_from_hash, DeployOutcome};
use sdkt_xdr::sign::{Ed25519Signer, Network};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

fn extract_jsonrpc_method(req: &str) -> Option<String> {
    let body_start = req.find("\r\n\r\n")?;
    let body = &req[body_start + 4..];
    let val: serde_json::Value = serde_json::from_str(body).ok()?;
    val.get("method")?.as_str().map(ToString::to_string)
}

/// Spawn a mock JSON-RPC server that satisfies the deploy flow and counts the
/// number of `simulateTransaction` and `sendTransaction` calls it receives.
fn spawn_mock() -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let sim_count = Arc::new(AtomicUsize::new(0));
    let send_count = Arc::new(AtomicUsize::new(0));
    let sim_c = sim_count.clone();
    let send_c = send_count.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            let body = match extract_jsonrpc_method(&req).as_deref() {
                Some("getLedgerEntries") => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                ),
                Some("simulateTransaction") => {
                    sim_c.fetch_add(1, Ordering::SeqCst);
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                    )
                }
                Some("sendTransaction") => {
                    send_c.fetch_add(1, Ordering::SeqCst);
                    r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeef1234","status":"PENDING","latestLedger":"100"}}"#.to_string()
                }
                Some("getTransaction") => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg=="}}"#.to_string()
                }
                _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string(),
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

    (format!("http://{addr}"), sim_count, send_count)
}

fn test_signer() -> (Ed25519Signer, String) {
    let signer = Ed25519Signer::from_seed(&[0x01u8; 32]);
    let source = format!(
        "{}",
        stellar_strkey::Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(
            signer.public_key_bytes_owned(),
        ))
    );
    (signer, source)
}

#[tokio::test]
async fn deploy_from_hash_creates_without_uploading() {
    let (url, sim_count, send_count) = spawn_mock();
    let client = SorobanRpcClient::new(&url);
    let (signer, source) = test_signer();

    let wasm_hash = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
    let salt = [0x42u8; 20];

    let outcome = deploy_contract_from_hash(
        &client,
        wasm_hash,
        &source,
        &signer,
        Network::Testnet,
        Some(salt),
        Vec::new(),
    )
    .await
    .expect("create-only deploy succeeds");

    match outcome {
        DeployOutcome::Success(res) => {
            // No upload happened: no upload hash, no upload fee, hash echoed back.
            assert_eq!(res.upload_hash, "");
            assert_eq!(res.upload_fee, 0);
            assert_eq!(res.wasm_hash, wasm_hash);
            assert_eq!(res.status, "SUCCESS");
            assert_eq!(res.total_fee, res.create_fee as u64);
        }
        other => panic!("expected Success, got {other:?}"),
    }

    // Exactly one create transaction: one simulate + one sendTransaction. The
    // full path would issue two of each (upload then create).
    assert_eq!(sim_count.load(Ordering::SeqCst), 1, "should simulate once");
    assert_eq!(send_count.load(Ordering::SeqCst), 1, "should submit once");
}

#[tokio::test]
async fn deploy_from_hash_matches_full_deploy_contract_id() {
    let salt = [0x42u8; 20];
    let (signer, source) = test_signer();

    // Full deploy first to obtain the on-chain wasm hash + derived contract id.
    let (url1, _, _) = spawn_mock();
    let client1 = SorobanRpcClient::new(&url1);
    let full = deploy_contract(
        &client1,
        b"\0asm\x01\0\0\0",
        &source,
        &signer,
        Network::Testnet,
        Some(salt),
    )
    .await
    .expect("full deploy succeeds");
    let (wasm_hash, full_contract_id) = match full {
        DeployOutcome::Success(res) => (res.wasm_hash, res.contract_id),
        other => panic!("expected Success, got {other:?}"),
    };

    // Create-only with the same salt + wasm hash must derive the same id.
    let (url2, _, _) = spawn_mock();
    let client2 = SorobanRpcClient::new(&url2);
    let create_only = deploy_contract_from_hash(
        &client2,
        &wasm_hash,
        &source,
        &signer,
        Network::Testnet,
        Some(salt),
        Vec::new(),
    )
    .await
    .expect("create-only deploy succeeds");
    let create_only_id = match create_only {
        DeployOutcome::Success(res) => res.contract_id,
        other => panic!("expected Success, got {other:?}"),
    };

    assert_eq!(
        create_only_id, full_contract_id,
        "same salt + wasm hash must derive the same contract id"
    );
}

fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_NETWORK_DIR", dir);
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd
}

#[test]
fn cli_deploy_rejects_both_wasm_and_wasm_hash() {
    let dir = tempfile::tempdir().unwrap();
    sdkt(dir.path())
        .args([
            "deploy",
            "--wasm",
            "contract.wasm",
            "--wasm-hash",
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "specify only one of --wasm or --wasm-hash",
        ));
}

#[test]
fn cli_deploy_requires_a_code_source() {
    let dir = tempfile::tempdir().unwrap();
    sdkt(dir.path())
        .args(["deploy"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "provide either --wasm <FILE> or --wasm-hash <HASH>",
        ));
}
