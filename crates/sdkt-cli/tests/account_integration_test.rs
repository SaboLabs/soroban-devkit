use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

fn add_mock_account_profile(dir: &std::path::Path, rpc_url: &str) {
    sdkt_isolated(dir)
        .args([
            "network",
            "add",
            "mocknet",
            "--rpc-url",
            rpc_url,
            "--passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success();
}

#[test]
fn test_account_format_json() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("account")
        .arg("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
        .arg("--format")
        .arg("json");

    let output = cmd.output().unwrap();
    assert!(output.status.success() || output.status.code().unwrap() == 1);
}

#[test]
fn test_account_invalid_format() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("account")
        .arg("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
        .arg("--format")
        .arg("xml");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Invalid format"));
}

#[test]
fn test_account_missing_profile() {
    let dir = tempdir().unwrap();

    sdkt_isolated(dir.path())
        .args([
            "account",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "--network-profile",
            "ghost",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn test_account_invalid_address() {
    let dir = tempdir().unwrap();
    let rpc_url = "http://127.0.0.1:1"; // Port 1 will refuse connection
    add_mock_account_profile(dir.path(), rpc_url);

    let output = sdkt_isolated(dir.path())
        .args(["account", "INVALID_ADDRESS", "--network-profile", "mocknet"])
        .output()
        .unwrap();

    // Should fail with clear error about invalid address
    assert!(!output.status.success(), "Should fail with invalid address");
}

#[test]
fn test_account_help_shows_format_options() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("account").arg("--help");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--format"))
        .stdout(predicate::str::contains("--network-profile"));
}

#[test]
fn test_account_json_structure() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("account")
        .arg("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
        .arg("--format")
        .arg("json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // If it succeeded (network available), verify JSON structure
    if output.status.success() {
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("Should be valid JSON");
        assert!(parsed.get("address").is_some(), "Missing address field");
        assert!(parsed.get("sequence").is_some(), "Missing sequence field");
        assert!(parsed.get("balances").is_some(), "Missing balances field");
        assert!(parsed.get("signers").is_some(), "Missing signers field");
    }
}

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use stellar_xdr::{
    AccountEntry, AccountEntryExt, AccountId, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limits,
    PublicKey, SequenceNumber, Signer, SignerKey, String32, StringM, Thresholds, Uint256, WriteXdr,
};

const MOCK_ACCOUNT_ADDRESS: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

fn read_request(sock: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = sock.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end]
                .lines()
                .filter_map(|l| l.split_once(':'))
                .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).to_string()
}

fn mock_horizon_json(address: &str) -> String {
    format!(
        r#"{{
  "id": "{address}",
  "sequence": "987654321",
  "balances": [
    {{
      "asset_type": "native",
      "balance": "1000.5000000"
    }},
    {{
      "asset_type": "credit_alphanum4",
      "asset_code": "USDC",
      "asset_issuer": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
      "balance": "250.0000000"
    }},
    {{
      "asset_type": "credit_alphanum12",
      "asset_code": "EURC12",
      "asset_issuer": "GDQOE23CFSUMSVQK4Y5JHPP04KNOFG023V64UK57R6Q5K42NOK477C2M",
      "balance": "75.1234567"
    }}
  ],
  "signers": [
    {{
      "key": "{address}",
      "type": "ed25519",
      "weight": 1
    }},
    {{
      "key": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
      "type": "hash_x",
      "weight": 2
    }},
    {{
      "key": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "type": "pre_auth_tx",
      "weight": 3
    }}
  ]
}}"#
    )
}

fn create_mock_account_entry_xdr(address: &str) -> String {
    let key = stellar_strkey::Strkey::from_string(address).unwrap();
    let pubkey = match key {
        stellar_strkey::Strkey::PublicKeyEd25519(pk) => pk.0,
        _ => panic!("Expected Ed25519"),
    };

    let hash_x_bytes = [0xa1u8; 32];
    let pre_auth_bytes = [0xb2u8; 32];
    let ed25519_bytes = [0xc3u8; 32];

    let account_entry = AccountEntry {
        account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pubkey))),
        balance: 990000000,
        seq_num: SequenceNumber(12345),
        num_sub_entries: 3,
        inflation_dest: None,
        flags: 0,
        home_domain: String32(StringM::default()),
        thresholds: Thresholds([1, 1, 1, 1]),
        signers: vec![
            Signer {
                key: SignerKey::Ed25519(Uint256(ed25519_bytes)),
                weight: 1,
            },
            Signer {
                key: SignerKey::HashX(Uint256(hash_x_bytes)),
                weight: 2,
            },
            Signer {
                key: SignerKey::PreAuthTx(Uint256(pre_auth_bytes)),
                weight: 3,
            },
        ]
        .try_into()
        .unwrap(),
        ext: AccountEntryExt::V0,
    };

    let ledger_entry = LedgerEntry {
        last_modified_ledger_seq: 100,
        data: LedgerEntryData::Account(account_entry),
        ext: LedgerEntryExt::V0,
    };

    let mut buf = Vec::new();
    let mut limited = stellar_xdr::Limited::new(&mut buf, Limits::none());
    ledger_entry.write_xdr(&mut limited).unwrap();
    STANDARD.encode(&buf)
}

#[test]
fn test_account_multi_asset_and_mixed_signers_pretty() {
    let dir = tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}");
    let address = MOCK_ACCOUNT_ADDRESS;
    let horizon_body = mock_horizon_json(address);

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_request(&mut sock);
            if req.is_empty() {
                continue;
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                horizon_body.len(),
                horizon_body
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });

    add_mock_account_profile(dir.path(), &url);

    let output = sdkt_isolated(dir.path())
        .args(["account", address, "--network-profile", "mocknet"])
        .output()
        .unwrap();

    assert!(output.status.success(), "Command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Multi-asset balances verified:
    assert!(stdout.contains("Asset: native"));
    assert!(stdout.contains("Balance: 1000.5000000"));
    assert!(stdout.contains(
        "Asset: USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5 (credit_alphanum4)"
    ));
    assert!(stdout.contains("Balance: 250.0000000"));
    assert!(stdout.contains("Asset: EURC12:GDQOE23CFSUMSVQK4Y5JHPP04KNOFG023V64UK57R6Q5K42NOK477C2M (credit_alphanum12)"));
    assert!(stdout.contains("Balance: 75.1234567"));

    // Complete typed signers verified:
    assert!(stdout.contains("Type: ed25519"));
    assert!(stdout.contains(&format!("Key: {address}")));
    assert!(stdout.contains("Type: hash_x"));
    assert!(
        stdout.contains("Key: abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
    );
    assert!(stdout.contains("Type: pre_auth_tx"));
    assert!(
        stdout.contains("Key: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
    );
}

#[test]
fn test_account_multi_asset_and_mixed_signers_json() {
    let dir = tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}");
    let address = MOCK_ACCOUNT_ADDRESS;
    let horizon_body = mock_horizon_json(address);

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_request(&mut sock);
            if req.is_empty() {
                continue;
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                horizon_body.len(),
                horizon_body
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });

    add_mock_account_profile(dir.path(), &url);

    let output = sdkt_isolated(dir.path())
        .args([
            "account",
            address,
            "--network-profile",
            "mocknet",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "Command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Valid JSON");

    assert_eq!(parsed["address"], address);
    assert_eq!(parsed["sequence"], "987654321");

    let balances = parsed["balances"].as_array().expect("balances array");
    assert_eq!(balances.len(), 3);
    assert_eq!(balances[0]["asset_type"], "native");
    assert_eq!(balances[0]["balance"], "1000.5000000");

    assert_eq!(balances[1]["asset_type"], "credit_alphanum4");
    assert_eq!(balances[1]["asset_code"], "USDC");
    assert_eq!(
        balances[1]["asset_issuer"],
        "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
    );
    assert_eq!(balances[1]["balance"], "250.0000000");

    assert_eq!(balances[2]["asset_type"], "credit_alphanum12");
    assert_eq!(balances[2]["asset_code"], "EURC12");
    assert_eq!(
        balances[2]["asset_issuer"],
        "GDQOE23CFSUMSVQK4Y5JHPP04KNOFG023V64UK57R6Q5K42NOK477C2M"
    );
    assert_eq!(balances[2]["balance"], "75.1234567");

    let signers = parsed["signers"].as_array().expect("signers array");
    assert_eq!(signers.len(), 3);
    assert_eq!(signers[0]["type"], "ed25519");
    assert_eq!(signers[0]["key"], address);
    assert_eq!(signers[0]["weight"], 1);

    assert_eq!(signers[1]["type"], "hash_x");
    assert_eq!(
        signers[1]["key"],
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
    );
    assert_eq!(signers[1]["weight"], 2);

    assert_eq!(signers[2]["type"], "pre_auth_tx");
    assert_eq!(
        signers[2]["key"],
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    );
    assert_eq!(signers[2]["weight"], 3);
}

#[test]
fn test_account_rpc_fallback_mixed_signers() {
    let dir = tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}");
    let address = MOCK_ACCOUNT_ADDRESS;
    let entry_xdr = create_mock_account_entry_xdr(address);

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let req = read_request(&mut sock);
            if req.is_empty() {
                continue;
            }
            if req.starts_with("GET ") {
                // Horizon endpoint returns 404 so it falls back to RPC
                let resp =
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.flush();
            } else {
                // RPC getLedgerEntries response
                let rpc_body = format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{entry_xdr}","lastModifiedLedgerSeq":100}}],"latestLedger":1000}}}}"#
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    rpc_body.len(),
                    rpc_body
                );
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.flush();
            }
        }
    });

    add_mock_account_profile(dir.path(), &url);

    let output = sdkt_isolated(dir.path())
        .args(["account", address, "--network-profile", "mocknet"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "RPC fallback command should succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Asset: native"));
    assert!(stdout.contains("Type: ed25519"));
    assert!(stdout.contains("Type: hash_x"));
    assert!(stdout.contains("Type: pre_auth_tx"));
}
