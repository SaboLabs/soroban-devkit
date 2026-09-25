//! Mock-RPC integration tests for post-deploy WASM verification ().
//!
//! A local TCP listener impersonates a Soroban RPC node, so the verification
//! path (`getLedgerEntries` -> on-chain WASM hash extraction) runs end-to-end
//! without a live network.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sdkt_rpc::client::SorobanRpcClient;
use sdkt_rpc::verify_deployed_wasm;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use stellar_xdr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ContractId, ExtensionPoint,
    Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limited, Limits, ScAddress,
    ScContractInstance, ScVal, WriteXdr,
};

/// Minimal valid WASM binary (magic + version 1).
const MINIMAL_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

/// Raw 32-byte hex contract id (accepted by the inspection path directly,
/// avoiding any StrKey setup).
const CONTRACT_ID_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn extract_jsonrpc_method(req: &str) -> Option<String> {
    let body_start = req.find("\r\n\r\n")?;
    let body = &req[body_start + 4..];
    let val: serde_json::Value = serde_json::from_str(body).ok()?;
    val.get("method")?.as_str().map(ToString::to_string)
}

/// Build the base64 `LedgerEntry` an RPC node returns for a contract instance
/// whose executable WASM hash is `wasm_hash`.
fn contract_instance_entry_b64(wasm_hash: [u8; 32]) -> String {
    let entry = LedgerEntry {
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
    };
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    let mut l = Limited::new(&mut cursor, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

/// Spawn a mock RPC node that answers `getLedgerEntries` with `entry_b64`.
/// Returns the bound base URL.
fn spawn_mock_rpc(entry_b64: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
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
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#,
                    entry_b64
                ),
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
    format!("http://{}", addr)
}

/// Hash of the fixture artifact, computed exactly the way the deploy flow does.
fn local_wasm_hash(wasm: &[u8]) -> [u8; 32] {
    let hash_hex = sdkt_wasm::parse_metadata(wasm)
        .expect("fixture wasm has parseable metadata")
        .hash;
    let bytes = hex::decode(hash_hex).expect("metadata hash is hex");
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

#[tokio::test]
async fn verification_reports_match_when_on_chain_hash_equals_artifact() {
    let local = local_wasm_hash(MINIMAL_WASM);
    let url = spawn_mock_rpc(contract_instance_entry_b64(local));
    let client = SorobanRpcClient::new(&url);

    let report = verify_deployed_wasm(&client, CONTRACT_ID_HEX, MINIMAL_WASM)
        .await
        .expect("verification completes");

    assert!(report.matches);
    assert_eq!(report.local_wasm_hash, hex::encode(local));
    assert_eq!(report.on_chain_wasm_hash, hex::encode(local));
}

#[tokio::test]
async fn verification_reports_mismatch_when_on_chain_hash_differs() {
    let mut on_chain = local_wasm_hash(MINIMAL_WASM);
    on_chain[0] ^= 0xff;
    let url = spawn_mock_rpc(contract_instance_entry_b64(on_chain));
    let client = SorobanRpcClient::new(&url);

    let report = verify_deployed_wasm(&client, CONTRACT_ID_HEX, MINIMAL_WASM)
        .await
        .expect("verification completes");

    assert!(!report.matches);
    assert_ne!(report.on_chain_wasm_hash, report.local_wasm_hash);
    assert_eq!(report.on_chain_wasm_hash, hex::encode(on_chain));
}
