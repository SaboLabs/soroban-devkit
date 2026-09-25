//! Integration tests for deploy fee breakdown exposure.
//!
//! Verifies:
//! - DeployResult includes upload_fee, create_fee, total_fee (u64)
//! - Overflow safety when component fees sum above u32::MAX
//! - format_pretty includes fee breakdown
//! - format_json includes fee fields in camelCase
//! - total_fee == upload_fee + create_fee
//! - DeployOutcome::Display includes fee breakdown
//! - RPC-level fee calculation and propagation through deploy_contract

use sdkt_rpc::client::SorobanRpcClient;
use sdkt_rpc::deploy::deploy_contract;
use sdkt_rpc::{format_json, format_pretty, DeployOutcome, DeployResult};
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

#[test]
fn deploy_result_contains_all_fee_fields() {
    let res = DeployResult {
        wasm_hash: "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20".into(),
        contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".into(),
        upload_hash: "a1b2c3d4e5f6".into(),
        create_hash: "f6e5d4c3b2a1".into(),
        status: "SUCCESS".into(),
        salt: "00112233445566778899aabbccddeeff00112233".into(),
        upload_fee: 100_500,
        create_fee: 250_200,
        total_fee: 350_700,
    };

    assert_eq!(res.upload_fee, 100_500);
    assert_eq!(res.create_fee, 250_200);
    assert_eq!(res.total_fee, 350_700);
    assert_eq!(res.total_fee, res.upload_fee as u64 + res.create_fee as u64);
}

#[test]
fn deploy_result_fee_overflow_safety() {
    let upload_fee = u32::MAX;
    let create_fee = u32::MAX;
    let total_fee = upload_fee as u64 + create_fee as u64;
    let res = DeployResult {
        wasm_hash: "abcd1234".into(),
        contract_id: "C123".into(),
        upload_hash: "up_tx_1".into(),
        create_hash: "cr_tx_1".into(),
        status: "SUCCESS".into(),
        salt: "00112233445566778899aabbccddeeff00112233".into(),
        upload_fee,
        create_fee,
        total_fee,
    };

    assert_eq!(res.upload_fee, u32::MAX);
    assert_eq!(res.create_fee, u32::MAX);
    assert_eq!(res.total_fee, 8_589_934_590);
    assert!(res.total_fee > u32::MAX as u64);
    assert_eq!(res.total_fee, res.upload_fee as u64 + res.create_fee as u64);
}

#[test]
fn deploy_pretty_output_displays_fee_breakdown() {
    let res = DeployResult {
        wasm_hash: "abcd1234".into(),
        contract_id: "C123".into(),
        upload_hash: "up_tx_1".into(),
        create_hash: "cr_tx_1".into(),
        status: "SUCCESS".into(),
        salt: "00112233445566778899aabbccddeeff00112233".into(),
        upload_fee: 1234,
        create_fee: 5678,
        total_fee: 6912,
    };

    let pretty = format_pretty(&res);

    assert!(
        pretty.contains("Upload Fee: 1234"),
        "pretty output missing Upload Fee: {}",
        pretty
    );
    assert!(
        pretty.contains("Create Fee: 5678"),
        "pretty output missing Create Fee: {}",
        pretty
    );
    assert!(
        pretty.contains("Total Fee: 6912"),
        "pretty output missing Total Fee: {}",
        pretty
    );
}

#[test]
fn deploy_json_output_includes_camel_case_fee_fields() {
    let res = DeployResult {
        wasm_hash: "abcd1234".into(),
        contract_id: "C123".into(),
        upload_hash: "up_tx_1".into(),
        create_hash: "cr_tx_1".into(),
        status: "SUCCESS".into(),
        salt: "00112233445566778899aabbccddeeff00112233".into(),
        upload_fee: 1234,
        create_fee: 5678,
        total_fee: 6912,
    };

    let json_str = format_json(&res);
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("valid json output");

    assert_eq!(parsed["uploadFee"], 1234);
    assert_eq!(parsed["createFee"], 5678);
    assert_eq!(parsed["totalFee"], 6912);
    assert_eq!(
        parsed["totalFee"].as_u64().unwrap(),
        parsed["uploadFee"].as_u64().unwrap() + parsed["createFee"].as_u64().unwrap()
    );
}

#[test]
fn deploy_outcome_display_includes_fees() {
    let res = DeployResult {
        wasm_hash: "abcd1234".into(),
        contract_id: "C123".into(),
        upload_hash: "up_tx_1".into(),
        create_hash: "cr_tx_1".into(),
        status: "SUCCESS".into(),
        salt: "00112233445566778899aabbccddeeff00112233".into(),
        upload_fee: 100,
        create_fee: 200,
        total_fee: 300,
    };

    let outcome = DeployOutcome::Success(res);
    let display_str = format!("{}", outcome);

    assert!(display_str.contains("Upload Fee: 100"));
    assert!(display_str.contains("Create Fee: 200"));
    assert!(display_str.contains("Total Fee: 300"));
}

#[tokio::test]
async fn test_deploy_contract_rpc_fee_propagation() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let sim_count = Arc::new(AtomicUsize::new(0));
    let sim_count_server = sim_count.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            let method = extract_jsonrpc_method(&req);
            let body = match method.as_deref() {
                Some("getLedgerEntries") => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                ),
                Some("simulateTransaction") => {
                    let count = sim_count_server.fetch_add(1, Ordering::SeqCst);
                    let min_resource_fee = if count == 0 { "150" } else { "350" };
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"{min_resource_fee}","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                    )
                }
                Some("sendTransaction") => {
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

    let client = SorobanRpcClient::new(&format!("http://{}", addr));
    let seed = [0x01u8; 32];
    let signer = Ed25519Signer::from_seed(&seed);
    let pubkey_bytes = signer.public_key_bytes_owned();
    let source_account =
        stellar_strkey::Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(pubkey_bytes))
            .to_string();

    let wasm_bytes = b"\0asm\x01\0\0\0";
    let salt = [0x42u8; 20];
    let network = Network::Testnet;

    let outcome = deploy_contract(
        &client,
        wasm_bytes,
        &source_account,
        &signer,
        network,
        Some(salt),
    )
    .await
    .expect("deployment succeeds");

    match outcome {
        DeployOutcome::Success(res) => {
            // Upload: 100 inclusion fee + 150 min_resource_fee = 250
            assert_eq!(res.upload_fee, 250);
            // Create: 100 inclusion fee + 350 min_resource_fee = 450
            assert_eq!(res.create_fee, 450);
            // Total: 250 + 450 = 700
            assert_eq!(res.total_fee, 700);
            assert_eq!(res.total_fee, res.upload_fee as u64 + res.create_fee as u64);
            assert_eq!(res.status, "SUCCESS");
            assert_eq!(res.upload_hash, "deadbeef1234");
            assert_eq!(res.create_hash, "deadbeef1234");
        }
        other => panic!("Expected DeployOutcome::Success, got: {:?}", other),
    }
}
