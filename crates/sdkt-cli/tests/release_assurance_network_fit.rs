//! Integration tests for `sdkt release-assurance` Network Fit section (#522).

use assert_cmd::Command;
use base64::Engine as _;
use predicates::str::contains;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;
use stellar_xdr::{ConfigSettingEntry, LedgerEntryData, Limits, WriteXdr};

const MINIMAL_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

fn wasm_path(wasm: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("candidate.wasm");
    std::fs::write(&path, wasm).expect("write wasm");
    (dir, path)
}

fn encode_max_size_entry(limit: u32) -> String {
    let data = LedgerEntryData::ConfigSetting(ConfigSettingEntry::ContractMaxSizeBytes(limit));
    let mut buf = Vec::new();
    data.write_xdr(&mut stellar_xdr::Limited::new(&mut buf, Limits::none()))
        .expect("encode XDR");
    base64::engine::general_purpose::STANDARD.encode(buf)
}

fn read_http_request_body(sock: &mut std::net::TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let n = sock.read(&mut chunk).expect("read HTTP request");
        assert!(n > 0, "unexpected EOF before HTTP header terminator");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]);
    let content_length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, val) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                val.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = sock.read(&mut chunk).expect("read HTTP body");
        assert!(n > 0, "unexpected EOF before full HTTP body");
        buf.extend_from_slice(&chunk[..n]);
    }
    buf[header_end..header_end + content_length].to_vec()
}

/// Spawn a mock Soroban RPC server responding to `getNetwork`, `getLatestLedger`,
/// and `getLedgerEntries` (returning `ContractMaxSizeBytes(limit)` when `Some(limit)`).
fn mock_capture_rpc(limit: Option<u32>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock rpc");
    let addr = listener.local_addr().unwrap();
    let entry_b64 = limit.map(encode_max_size_entry);

    thread::spawn(move || {
        // Handle up to 6 sequential requests on separate connections.
        for _ in 0..6 {
            let Ok((mut sock, _)) = listener.accept() else {
                break;
            };
            let body = read_http_request_body(&mut sock);
            let req_json: serde_json::Value =
                serde_json::from_slice(&body).expect("valid JSON-RPC request");
            let method = req_json["method"].as_str().expect("JSON-RPC method");
            let id = req_json["id"].clone();

            let result = match method {
                "getNetwork" => serde_json::json!({
                    "passphrase": "Test SDF Network ; September 2015",
                    "protocolVersion": 23
                }),
                "getLatestLedger" => serde_json::json!({
                    "id": "abc",
                    "protocolVersion": 23,
                    "sequence": 100
                }),
                "getLedgerEntries" => {
                    let entries = match &entry_b64 {
                        Some(xdr) => serde_json::json!([{
                            "key": "AAAAAAAA",
                            "xdr": xdr,
                            "lastModifiedLedgerSeq": 99
                        }]),
                        None => serde_json::json!([]),
                    };
                    serde_json::json!({
                        "entries": entries,
                        "latestLedger": 100
                    })
                }
                _ => serde_json::json!({}),
            };

            let payload = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    format!("http://{addr}")
}

#[test]
fn network_fit_passes_when_candidate_fits_and_at_exact_limit() {
    let (_dir, wasm) = wasm_path(MINIMAL_WASM);
    // 1. Candidate (8 bytes) fits within limit (65536 bytes).
    let rpc_url = mock_capture_rpc(Some(65536));
    let out = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "release-assurance",
            "--wasm",
            wasm.to_str().unwrap(),
            "--rpc-url",
            &rpc_url,
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["network_fit"]["status"], "PASS");
    assert_eq!(v["network_fit"]["candidate_size_bytes"], 8);
    assert_eq!(v["network_fit"]["max_contract_size_bytes"], 65536);

    // 2. Candidate (8 bytes) exactly at limit (8 bytes) passes.
    let rpc_exact = mock_capture_rpc(Some(8));
    let out_exact = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "release-assurance",
            "--wasm",
            wasm.to_str().unwrap(),
            "--rpc-url",
            &rpc_exact,
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v_exact: serde_json::Value = serde_json::from_slice(&out_exact).unwrap();
    assert_eq!(v_exact["network_fit"]["status"], "PASS");
}

#[test]
fn network_fit_fails_when_candidate_exceeds_limit_naming_both_counts() {
    let (_dir, wasm) = wasm_path(MINIMAL_WASM); // 8 bytes
    let rpc_url = mock_capture_rpc(Some(4));
    let out = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "release-assurance",
            "--wasm",
            wasm.to_str().unwrap(),
            "--rpc-url",
            &rpc_url,
            "--format",
            "json",
        ])
        .assert()
        .failure()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["network_fit"]["status"], "FAIL");
    assert_eq!(v["release_status"], "FAIL");
    let detail = v["network_fit"]["detail"].as_str().unwrap();
    assert!(
        detail.contains('8'),
        "detail should name candidate size: {detail}"
    );
    assert!(detail.contains('4'), "detail should name limit: {detail}");
    assert!(v["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r.as_str() == Some("Network Fit: FAIL")));

    // Pretty output also displays Network Fit FAIL and both byte counts.
    let rpc_pretty = mock_capture_rpc(Some(4));
    Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "release-assurance",
            "--wasm",
            wasm.to_str().unwrap(),
            "--rpc-url",
            &rpc_pretty,
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(contains("Network Fit     FAIL"))
        .stdout(contains("8 bytes"))
        .stdout(contains("4 bytes"));
}

#[test]
fn network_fit_skips_when_unreachable_or_unavailable_without_affecting_verdict() {
    let (_dir, wasm) = wasm_path(MINIMAL_WASM);

    // 1. Unreachable RPC -> SKIPPED, exit 0, overall verdict unaffected.
    let out = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "release-assurance",
            "--wasm",
            wasm.to_str().unwrap(),
            "--rpc-url",
            "http://127.0.0.1:1",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["network_fit"]["status"], "SKIPPED");
    assert_eq!(v["release_status"], "REVIEW");

    // 2. Reachable RPC that omits ContractMaxSizeBytes -> SKIPPED, exit 0.
    let rpc_missing = mock_capture_rpc(None);
    let out_missing = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "release-assurance",
            "--wasm",
            wasm.to_str().unwrap(),
            "--rpc-url",
            &rpc_missing,
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v_missing: serde_json::Value = serde_json::from_slice(&out_missing).unwrap();
    assert_eq!(v_missing["network_fit"]["status"], "SKIPPED");
}
