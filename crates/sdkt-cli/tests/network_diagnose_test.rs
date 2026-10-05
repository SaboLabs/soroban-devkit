//! Network diagnosis tests — hermetic.
//!
//! `sdkt network diagnose` is proven against a local mock that answers BOTH
//! the Soroban JSON-RPC probes (`getHealth`, `getNetwork`, `getLatestLedger`)
//! and the derived Horizon paths (`GET /`, `GET /ledgers/{seq}`) on one
//! listener — `horizon_url_for_endpoint` keeps the host for custom endpoints,
//! so the mock URL doubles as RPC and Horizon. Covers:
//!
//! - identity match + protocol agreement + limits available → ok, exit 0
//! - passphrase mismatch → degraded, exit 1
//! - protocol disagreement → degraded, exit 1
//! - Horizon down → limits unavailable (never fabricated), no values
//! - RPC unreachable → exit 1, no report
//! - futurenet as an explicit read-only verification target
//! - `--help` documents the read-only boundary

use assert_cmd::Command;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";
const MAINNET_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";
const FUTURENET_PASSPHRASE: &str = "Test SDF Future Network ; October 2022";
const CONTRACT_HEX: &str = "09ba7d2a24a36c9de487f43ab000000000000000000000000000000000000000";

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").expect("sdkt binary built")
}

fn read_http_request(sock: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = sock.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data);
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end]
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

/// Mock RPC + Horizon on one listener; `horizon_ok` toggles the Horizon paths.
fn mock_endpoint(passphrase: &str, ledger_protocol: u32, horizon_ok: bool) -> String {
    let passphrase = passphrase.to_string();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let instance_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(CONTRACT_HEX.to_string())).unwrap();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let request = read_http_request(&mut sock);
            let first_line = request.lines().next().unwrap_or("");
            let mut head = first_line.split_whitespace();
            let verb = head.next().unwrap_or("");
            let path = head.next().unwrap_or("").to_string();
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or("");
            let request_json: Value = serde_json::from_str(body).unwrap_or_default();
            let method = request_json["method"].as_str().unwrap_or("").to_string();

            // JSON-RPC is POST /; Horizon is GET / and GET /ledgers/{seq}.
            // Routing by verb matters: a POST / would otherwise be answered
            // with the Horizon server-info body and every probe would fail.
            let (status_line, response) = if verb == "GET" && path == "/" {
                if horizon_ok {
                    (
                        "200 OK",
                        serde_json::json!({
                            "horizon_version": "29.0.0-mock",
                            "core_version": "stellar-core 29.0.0 (mock)",
                            "network_passphrase": TESTNET_PASSPHRASE,
                            "current_protocol_version": ledger_protocol,
                            "supported_protocol_version": ledger_protocol,
                            "history_latest_ledger": 10000,
                        })
                        .to_string(),
                    )
                } else {
                    (
                        "500 Internal Server Error",
                        r#"{"error":"not available"}"#.to_string(),
                    )
                }
            } else if path.starts_with("/ledgers/") {
                if horizon_ok {
                    (
                        "200 OK",
                        serde_json::json!({
                            "sequence": 10000,
                            "protocol_version": ledger_protocol,
                            "base_fee_in_stroops": 100,
                            "base_reserve_in_stroops": 5000000,
                            "max_tx_set_size": 200,
                        })
                        .to_string(),
                    )
                } else {
                    ("500 Internal Server Error", "{}".to_string())
                }
            } else {
                let body = match method.as_str() {
                    "getHealth" => r#"{"jsonrpc":"2.0","id":1,"result":{"status":"healthy"}}"#
                        .to_string(),
                    "getNetwork" => format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"passphrase":"{passphrase}","protocolVersion":29}}}}"#
                    ),
                    "getLatestLedger" => format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"id":"mock","protocolVersion":{ledger_protocol},"sequence":10000}}}}"#
                    ),
                    "getLedgerEntries" => format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{instance_key}","xdr":"AAAA","lastModifiedLedgerSeq":1}}],"latestLedger":10000}}}}"#
                    ),
                    _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                        .to_string(),
                };
                ("200 OK", body)
            };

            let http = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            let _ = sock.write_all(http.as_bytes());
        }
    });

    url
}

/// Run the diagnosis against a mock URL under a configured passphrase.
///
/// The target is configured with `--rpc-url` + `--network-passphrase` (never
/// `--network`, which clap conflicts with `--rpc-url`); the canonical network
/// name is derived from the passphrase exactly as `verify` does.
fn diagnose(url: &str, configured_passphrase: &str) -> (i32, Value) {
    let out = sdkt()
        .args([
            "network",
            "diagnose",
            "--format",
            "json",
            "--rpc-url",
            url,
            "--network-passphrase",
            configured_passphrase,
        ])
        .output()
        .expect("run sdkt");
    let code = out.status.code().unwrap_or(-1);
    let report: Value = String::from_utf8_lossy(&out.stdout)
        .parse()
        .expect("diagnose always emits a JSON report");
    (code, report)
}

#[test]
fn healthy_target_is_ok_with_observed_limits() {
    let url = mock_endpoint(TESTNET_PASSPHRASE, 29, true);
    let (code, report) = diagnose(&url, TESTNET_PASSPHRASE);
    assert_eq!(code, 0);
    assert_eq!(report["status"], "ok");
    assert_eq!(report["network"], "testnet");
    assert_eq!(report["identity"]["status"], "match");
    assert_eq!(report["protocol"]["status"], "consistent");
    assert_eq!(report["resource_limits"]["status"], "available");
    assert_eq!(report["resource_limits"]["source"], "horizon ledger 10000");
    let v = &report["resource_limits"]["values"];
    assert_eq!(v["base_fee_in_stroops"], 100);
    assert_eq!(v["base_reserve_in_stroops"], 5000000);
    assert_eq!(v["max_tx_set_size"], 200);
    assert_eq!(v["protocol_version"], 29);
}

#[test]
fn passphrase_mismatch_is_degraded_and_exits_one() {
    // Endpoint claims mainnet while configured for testnet: the classic
    // wrong-network foot-gun, reported — never silently accepted.
    let url = mock_endpoint(MAINNET_PASSPHRASE, 29, true);
    let (code, report) = diagnose(&url, TESTNET_PASSPHRASE);
    assert_eq!(code, 1);
    assert_eq!(report["status"], "degraded");
    assert_eq!(report["identity"]["status"], "mismatch");
    assert_eq!(
        report["identity"]["endpoint_passphrase"],
        MAINNET_PASSPHRASE
    );
}

#[test]
fn protocol_disagreement_is_degraded() {
    // getNetwork says 29, getLatestLedger + Horizon say 28 → inconsistent.
    let url = mock_endpoint(TESTNET_PASSPHRASE, 28, true);
    let (code, report) = diagnose(&url, TESTNET_PASSPHRASE);
    assert_eq!(code, 1);
    assert_eq!(report["status"], "degraded");
    assert_eq!(report["protocol"]["status"], "inconsistent");
    assert_eq!(report["protocol"]["rpc_get_network"], 29);
    assert_eq!(report["protocol"]["rpc_get_ledger"], 28);
    assert_eq!(report["protocol"]["horizon"], 28);
}

#[test]
fn horizon_down_reports_limits_unavailable_never_fabricated() {
    let url = mock_endpoint(TESTNET_PASSPHRASE, 29, false);
    let (code, report) = diagnose(&url, TESTNET_PASSPHRASE);
    // RPC itself is healthy; only Horizon is missing.
    assert_eq!(code, 0);
    assert_eq!(report["identity"]["status"], "match");
    assert_eq!(report["resource_limits"]["status"], "unavailable");
    assert!(report["resource_limits"].get("values").is_none());
    assert!(report["resource_limits"].get("source").is_none());
    assert!(report["resource_limits"]["reason"]
        .as_str()
        .is_some_and(|r| r.contains("Horizon")));
}

#[test]
fn unreachable_rpc_is_reported_unreachable_and_exits_one() {
    // A dead endpoint still produces the report (with status `unreachable`,
    // identity `unknown`, limits `unavailable`) and exits 1 — consumers gate
    // on the exit code, readers get the reason. No verdict is fabricated.
    let (code, report) = diagnose("http://127.0.0.1:1", TESTNET_PASSPHRASE);
    assert_eq!(code, 1);
    assert_eq!(report["status"], "unreachable");
    assert_eq!(report["identity"]["status"], "unknown");
    assert_eq!(report["resource_limits"]["status"], "unavailable");
    assert!(report["resource_limits"].get("values").is_none());
}

#[test]
fn futurenet_is_an_explicit_read_only_target() {
    let url = mock_endpoint(FUTURENET_PASSPHRASE, 29, true);
    let (code, report) = diagnose(&url, FUTURENET_PASSPHRASE);
    assert_eq!(code, 0);
    assert_eq!(report["status"], "ok");
    assert_eq!(report["network"], "futurenet");
    assert_eq!(report["identity"]["status"], "match");
}

#[test]
fn help_documents_the_read_only_boundary() {
    sdkt()
        .args(["network", "diagnose", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--network"))
        .stdout(predicates::str::contains(
            "Never signs, submits, or deploys",
        ));
}
