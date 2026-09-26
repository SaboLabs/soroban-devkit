//! Integration tests for `sdkt project deploy --salt`.
//!
//! Verifies the flag is validated and forwarded to `deploy_contract` instead of
//! being silently discarded. CI-safe: the forwarding test talks to a local mock
//! JSON-RPC server, never a live network.

use assert_cmd::Command;
use base64::Engine;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use stellar_xdr::{
    ContractIdPreimage, HostFunction, Limits, OperationBody, ReadXdr, TransactionEnvelope,
};

const VALID_SALT_HEX: &str = "00112233445566778899aabbccddeeff00112233";
const VALID_SALT_BYTES: [u8; 20] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
    0x00, 0x11, 0x22, 0x33,
];
const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

// Canned RPC payloads, same as the `deploy_contract` fee-propagation test.
const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.current_dir(dir);
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-project-salt-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        tag
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn project_deploy_rejects_invalid_salt() {
    let dir = temp_dir("nonhex");
    sdkt_isolated(&dir)
        .args(["project", "deploy", "--salt", "not_a_hex_string!"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid --salt"));
}

#[test]
fn project_deploy_rejects_wrong_length_salt() {
    let dir = temp_dir("len");
    sdkt_isolated(&dir)
        .args(["project", "deploy", "--salt", "00112233"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must be 20-byte hex"));
}

#[test]
fn project_deploy_help_shows_salt_without_string_default() {
    let dir = temp_dir("help");
    sdkt_isolated(&dir)
        .args(["project", "deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--salt"))
        .stdout(predicate::str::contains("Auto-generated if omitted"))
        .stdout(predicate::str::contains("[default: deploy]").not());
}

// ---------- Salt forwarding boundary ----------

/// Read one HTTP request (headers plus a `Content-Length` body) and return its body.
fn read_http_body(sock: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = sock.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(header_end) = text.find("\r\n\r\n") {
            let content_length = text[..header_end]
                .lines()
                .find_map(|l| {
                    let (name, value) = l.split_once(':')?;
                    if !name.trim().eq_ignore_ascii_case("content-length") {
                        return None;
                    }
                    value.trim().parse::<usize>().ok()
                })
                .unwrap_or(0);
            if buf.len() >= header_end + 4 + content_length {
                return String::from_utf8_lossy(&buf[header_end + 4..]).to_string();
            }
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// Return the 20-byte salt of a create-contract envelope, or `None` for any
/// other transaction (e.g. the WASM upload).
fn create_contract_salt(envelope_b64: &str) -> Option<[u8; 20]> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(envelope_b64)
        .ok()?;
    let env = TransactionEnvelope::from_xdr(bytes, Limits::none()).ok()?;
    let ops = match env {
        TransactionEnvelope::Tx(e) => e.tx.operations,
        _ => return None,
    };
    ops.iter().find_map(|op| {
        let OperationBody::InvokeHostFunction(invoke) = &op.body else {
            return None;
        };
        let preimage = match &invoke.host_function {
            HostFunction::CreateContract(args) => &args.contract_id_preimage,
            HostFunction::CreateContractV2(args) => &args.contract_id_preimage,
            _ => return None,
        };
        let ContractIdPreimage::Address(from_address) = preimage else {
            return None;
        };
        // The 20-byte salt is left-aligned and zero-padded into a Uint256.
        let padded = from_address.salt.0;
        assert!(
            padded[20..].iter().all(|b| *b == 0),
            "salt padding must be zero"
        );
        let mut salt = [0u8; 20];
        salt.copy_from_slice(&padded[..20]);
        Some(salt)
    })
}

/// Start a mock Soroban RPC server. Every create-contract salt submitted via
/// `sendTransaction` is recorded, which is exactly the `user_salt` that
/// `deploy_contract` used (a `None` argument would record a random salt).
fn spawn_mock_rpc() -> (String, Arc<Mutex<Vec<[u8; 20]>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let recorded_server = recorded.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req: serde_json::Value =
                serde_json::from_str(&read_http_body(&mut sock)).unwrap_or_default();
            let body = match req["method"].as_str() {
                Some("getLedgerEntries") => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                ),
                Some("simulateTransaction") => format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"100","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                ),
                Some("sendTransaction") => {
                    if let Some(salt) = req["params"]["transaction"]
                        .as_str()
                        .and_then(create_contract_salt)
                    {
                        recorded_server.lock().unwrap().push(salt);
                    }
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

    (url, recorded)
}

/// Minimal one-contract project: `.sdkt.toml` plus a built WASM artifact where
/// `resolve_project` looks for it.
fn write_project_fixture(dir: &std::path::Path) {
    std::fs::write(
        dir.join(".sdkt.toml"),
        "[contracts.token]\npath = \"contracts/token\"\n",
    )
    .unwrap();
    let release = dir.join("contracts/token/target/wasm32-unknown-unknown/release");
    std::fs::create_dir_all(&release).unwrap();
    std::fs::write(release.join("token.wasm"), b"\0asm\x01\0\0\0").unwrap();
}

#[test]
fn project_deploy_forwards_decoded_salt_to_deploy_contract() {
    let dir = temp_dir("forward");
    write_project_fixture(&dir);
    sdkt_isolated(&dir)
        .args(["identity", "generate", "deployer"])
        .assert()
        .success();
    sdkt_isolated(&dir)
        .args(["identity", "default", "deployer"])
        .assert()
        .success();

    let (rpc_url, recorded) = spawn_mock_rpc();

    let output = sdkt_isolated(&dir)
        .args([
            "project",
            "--rpc-url",
            rpc_url.as_str(),
            "--network-passphrase",
            TESTNET_PASSPHRASE,
            "deploy",
            "--salt",
            VALID_SALT_HEX,
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout should be JSON ({}): {}",
            e,
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(json["status"], "success");
    assert!(json["contracts_deployed"]["token"].is_string());

    // `create_contract` may submit the same signed envelope more than once
    // (send, then submit-and-wait), so check every submission, not a count.
    let salts = recorded.lock().unwrap().clone();
    assert!(!salts.is_empty(), "expected a create-contract submission");
    for salt in salts {
        assert_eq!(
            Some(salt),
            Some(VALID_SALT_BYTES),
            "deploy_contract must receive the decoded --salt, not a generated one"
        );
    }
}
