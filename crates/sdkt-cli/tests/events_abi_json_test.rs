//! Regression tests for ABI-aware event JSON output.
//!
//! `sdkt events --abi --format json` must preserve the raw event payload
//! (`topics` and `value`) while still emitting the ABI-decoded representation.
//! Before the fix the ABI-aware JSON branch dropped `topics`/`value`, so
//! automation using `--abi --format json` lost raw event information that the
//! non-ABI JSON path and the pretty path both retained.
//!
//! These tests are hermetic: a local mock RPC endpoint serves the
//! `getLatestLedger` and `getEvents` JSON-RPC responses, and the ABI is a
//! checked-in WASM fixture. No network access is required.

use assert_cmd::Command;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use stellar_xdr::{ScSymbol, ScVal};

/// Build the `sdkt` binary under test with an isolated network store so a
/// developer's saved profiles can never leak into the run.
fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    let dir = std::env::temp_dir().join(format!(
        "sdkt-events-abi-json-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);
    cmd.env("SDKT_NETWORK_DIR", &dir);
    cmd
}

const ABI_WASM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_new.wasm");
const CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const PASSPHRASE: &str = "Test SDF Network ; September 2015";

/// A mock JSON-RPC server that answers `getLatestLedger` and `getEvents`.
///
/// It accepts multiple sequential connections (the client calls
/// `getLatestLedger` first, then `getEvents`) and dispatches on the request
/// body so each method gets a well-formed response.
fn mock_rpc_server(events_result: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let events_result = events_result.to_string();

    thread::spawn(move || {
        for _ in 0..4 {
            let (mut sock, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();

            let body = if request.contains("getEvents") {
                format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{events_result}}}")
            } else {
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"id\":\"mock\",\"protocolVersion\":22,\"sequence\":12345}}".to_string()
            };

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(response.as_bytes());
        }
    });

    thread::sleep(Duration::from_millis(50));
    url
}

/// Encode an `ScVal` as the base64 XDR string the RPC wire format uses.
fn scval_b64(val: &ScVal) -> String {
    sdkt_xdr::scval_to_base64(val).expect("scval encodes to base64 XDR")
}

/// The exact raw topics/value the mock RPC returns; the CLI output must retain
/// these byte-for-byte.
fn raw_event_payload() -> (Vec<String>, String) {
    let topics = vec![
        scval_b64(&ScVal::Symbol(ScSymbol("Mint".try_into().unwrap()))),
        scval_b64(&ScVal::U64(100)),
    ];
    let value = scval_b64(&ScVal::U64(500));
    (topics, value)
}

/// `getEvents` result JSON with a single event carrying the raw payload.
fn events_result(topics: &[String], value: &str) -> String {
    let topics_json = serde_json::to_string(topics).unwrap();
    format!(
        "{{\"events\":[{{\"ledger\":12345,\"contractId\":\"{CONTRACT_ID}\",\"topic\":{topics_json},\"value\":\"{value}\"}}]}}"
    )
}

fn run_events(url: &str, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "events",
        CONTRACT_ID,
        "--rpc-url",
        url,
        "--network-passphrase",
        PASSPHRASE,
    ];
    args.extend_from_slice(extra);
    sdkt().args(args).output().expect("sdkt runs")
}

fn stdout_json(output: &std::process::Output) -> Value {
    let text = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "expected JSON stdout, got error {e}; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

// ---------------------------------------------------------------------------
// ABI-aware JSON output
// ---------------------------------------------------------------------------

#[test]
fn abi_json_preserves_raw_topics_and_value() {
    let (topics, value) = raw_event_payload();
    let url = mock_rpc_server(&events_result(&topics, &value));

    let out = run_events(&url, &["--abi", ABI_WASM, "--format", "json"]);
    assert!(
        out.status.success(),
        "events --abi --format json failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let json = stdout_json(&out);
    let first = json
        .as_array()
        .expect("array of events")
        .first()
        .expect("one event");

    assert_eq!(
        first.get("topics").and_then(|t| t.as_array()),
        Some(
            &topics
                .iter()
                .map(|t| Value::String(t.clone()))
                .collect::<Vec<_>>()
        ),
        "ABI-aware JSON must retain the raw topics unchanged"
    );
    assert_eq!(
        first.get("value").and_then(|v| v.as_str()),
        Some(value.as_str()),
        "ABI-aware JSON must retain the raw value unchanged"
    );
}

#[test]
fn abi_json_keeps_decoded_information() {
    let (topics, value) = raw_event_payload();
    let url = mock_rpc_server(&events_result(&topics, &value));

    let out = run_events(&url, &["--abi", ABI_WASM, "--format", "json"]);
    assert!(out.status.success());

    let json = stdout_json(&out);
    let first = json.as_array().unwrap().first().unwrap();

    let decoded = first
        .get("decoded")
        .and_then(|d| d.as_array())
        .expect("ABI-aware JSON must still include `decoded`");
    assert!(
        !decoded.is_empty(),
        "decoded representation should not be empty for the supplied payload"
    );
    for entry in decoded {
        assert!(entry.get("raw").is_some(), "decoded entry needs `raw`");
        assert!(entry.get("label").is_some(), "decoded entry needs `label`");
        assert!(
            entry.get("matched_type").is_some(),
            "decoded entry needs `matched_type`"
        );
        assert!(
            entry.get("fields").is_some(),
            "decoded entry needs `fields`"
        );
    }
}

// ---------------------------------------------------------------------------
// Compatibility: non-ABI JSON and pretty output
// ---------------------------------------------------------------------------

#[test]
fn non_abi_json_output_remains_compatible() {
    let (topics, value) = raw_event_payload();
    let url = mock_rpc_server(&events_result(&topics, &value));

    let out = run_events(&url, &["--format", "json"]);
    assert!(out.status.success());

    let json = stdout_json(&out);
    let first = json.as_array().unwrap().first().unwrap();

    assert_eq!(
        first.get("contract_id").and_then(|c| c.as_str()),
        Some(CONTRACT_ID)
    );
    assert_eq!(first.get("ledger").and_then(|l| l.as_u64()), Some(12345));
    assert_eq!(
        first.get("topics").and_then(|t| t.as_array()),
        Some(
            &topics
                .iter()
                .map(|t| Value::String(t.clone()))
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(
        first.get("value").and_then(|v| v.as_str()),
        Some(value.as_str())
    );
    assert!(
        first.get("decoded").is_none(),
        "non-ABI JSON must not gain a `decoded` field"
    );
}

#[test]
fn pretty_output_unchanged_without_abi() {
    let (topics, value) = raw_event_payload();
    let url = mock_rpc_server(&events_result(&topics, &value));

    let out = run_events(&url, &[]);
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Contract Events:"));
    assert!(stdout.contains("Topics:"));
    assert!(stdout.contains("Value:"));
}

#[test]
fn pretty_output_unchanged_with_abi() {
    let (topics, value) = raw_event_payload();
    let url = mock_rpc_server(&events_result(&topics, &value));

    let out = run_events(&url, &["--abi", ABI_WASM]);
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Contract Events (ABI-decoded):"));
    assert!(stdout.contains("Topics:"));
    assert!(stdout.contains("Value:"));
    assert!(stdout.contains("Decoded:"));
}
