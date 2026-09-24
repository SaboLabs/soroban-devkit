use assert_cmd::Command;
use tempfile::NamedTempFile;

const TEST_SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const TEST_CONTRACT: &str = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";

#[test]
fn test_cli_tx_build_success() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("build")
        .arg("--source")
        .arg(TEST_SOURCE)
        .arg("--sequence")
        .arg("1")
        .arg("--contract")
        .arg(TEST_CONTRACT)
        .arg("--function")
        .arg("hello")
        .assert();
    assert.success().stdout(predicates::str::contains("AAAA"));
}

#[test]
fn test_cli_tx_build_typed_args() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("build")
        .arg("--source")
        .arg(TEST_SOURCE)
        .arg("--sequence")
        .arg("1")
        .arg("--contract")
        .arg(TEST_CONTRACT)
        .arg("--function")
        .arg("transfer")
        .arg("--arg")
        .arg("u32:100")
        .arg("--arg")
        .arg("string:hello")
        .arg("--arg")
        .arg("bool:true")
        .arg("--format")
        .arg("json")
        .assert();
    assert
        .success()
        .stdout(predicates::str::contains(r#""envelope": "AAAA"#));
}

#[test]
fn test_cli_tx_build_invalid_arg_format() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("build")
        .arg("--source")
        .arg(TEST_SOURCE)
        .arg("--sequence")
        .arg("1")
        .arg("--contract")
        .arg(TEST_CONTRACT)
        .arg("--function")
        .arg("transfer")
        .arg("--arg")
        .arg("unknown_type:100")
        .assert();
    assert.failure();
}

#[test]
fn test_cli_tx_build_json() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("build")
        .arg("--source")
        .arg(TEST_SOURCE)
        .arg("--sequence")
        .arg("1")
        .arg("--contract")
        .arg(TEST_CONTRACT)
        .arg("--function")
        .arg("hello")
        .arg("--format")
        .arg("json")
        .assert();
    assert
        .success()
        .stdout(predicates::str::contains(r#""envelope": "AAAA"#));
}

#[test]
fn test_cli_tx_build_invalid_source() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("build")
        .arg("--source")
        .arg("invalid")
        .arg("--sequence")
        .arg("1")
        .arg("--contract")
        .arg(TEST_CONTRACT)
        .arg("--function")
        .arg("hello")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("Error building transaction"));
}

#[test]
fn test_cli_tx_build_output_file() {
    let temp = NamedTempFile::new().unwrap();
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("build")
        .arg("--source")
        .arg(TEST_SOURCE)
        .arg("--sequence")
        .arg("1")
        .arg("--contract")
        .arg(TEST_CONTRACT)
        .arg("--function")
        .arg("hello")
        .arg("--output")
        .arg(temp.path())
        .assert();

    assert
        .success()
        .stdout(predicates::str::contains("written to"));
    let content = std::fs::read_to_string(temp.path()).unwrap();
    assert!(content.starts_with("AAAA"));
}

// An unreachable RPC endpoint: the port is valid syntactically but nothing
// listens there, so any network resolution attempt fails fast (connection
// refused) without depending on a live testnet.
const UNREACHABLE_RPC: &str = "http://127.0.0.1:1";

#[test]
fn test_cli_tx_build_explicit_sequence_never_touches_network() {
    // With an explicit `--sequence`, the build must succeed even when the RPC
    // endpoint is unreachable — the override path must not perform any lookup.
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("--rpc-url")
        .arg(UNREACHABLE_RPC)
        .arg("build")
        .arg("--source")
        .arg(TEST_SOURCE)
        .arg("--sequence")
        .arg("42")
        .arg("--contract")
        .arg(TEST_CONTRACT)
        .arg("--function")
        .arg("hello")
        .assert();
    assert.success().stdout(predicates::str::contains("AAAA"));
}

#[test]
fn test_cli_tx_build_auto_sequence_reports_clean_rpc_error() {
    // Without `--sequence`, the build resolves the sequence from the network.
    // When the RPC is unreachable it must surface a clear error and exit
    // non-zero — never panic and never silently build with a bogus sequence.
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("tx")
        .arg("--rpc-url")
        .arg(UNREACHABLE_RPC)
        .arg("build")
        .arg("--source")
        .arg(TEST_SOURCE)
        .arg("--contract")
        .arg(TEST_CONTRACT)
        .arg("--function")
        .arg("hello")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("resolving sequence"));
}

// ── Network-aware fee estimation ─────────────────────────────────────────────
//
// `tx build` is offline by default. When the operator names a network, it
// simulates the invocation and adopts the resource fee the network reports, so
// the documented build -> sign -> submit workflow stops producing envelopes
// that are rejected with txINSUFFICIENT_FEE. An explicit `--fee` always wins.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use stellar_xdr::{Limits, ReadXdr, TransactionEnvelope, TransactionExt};

const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

fn extract_jsonrpc_method(req: &str) -> Option<String> {
    let body_start = req.find("\r\n\r\n")?;
    let body = &req[body_start + 4..];
    let val: serde_json::Value = serde_json::from_str(body).ok()?;
    val.get("method")?.as_str().map(ToString::to_string)
}

/// Mock RPC that answers `simulateTransaction` with a fixed resource fee and
/// counts how many times it was asked.
fn mock_rpc(min_resource_fee: &'static str) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let sim_count = Arc::new(AtomicUsize::new(0));
    let counter = sim_count.clone();

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
                Some("simulateTransaction") => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"{min_resource_fee}","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                    )
                }
                _ => r#"{"jsonrpc":"2.0","id":1,"result":{}}"#.to_string(),
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    (format!("http://{addr}"), sim_count)
}

/// Fee baked into a base64 envelope.
fn envelope_fee(b64: &str) -> u32 {
    match TransactionEnvelope::from_xdr_base64(b64.trim(), Limits::none()).unwrap() {
        TransactionEnvelope::Tx(env) => env.tx.fee,
        _ => panic!("expected a V1 transaction envelope"),
    }
}

/// Instruction count from a base64 envelope's Soroban resources, if it has any.
///
/// The offline builder emits `TransactionExt::V0` — no Soroban data at all, so
/// the envelope is not merely under-priced but structurally unsubmittable.
/// Adopting simulation switches it to `V1` carrying the reported footprint
/// (1000 instructions, see `SOROBAN_DATA_XDR`).
fn envelope_instructions(b64: &str) -> Option<u32> {
    match TransactionEnvelope::from_xdr_base64(b64.trim(), Limits::none()).unwrap() {
        TransactionEnvelope::Tx(env) => match env.tx.ext {
            TransactionExt::V1(data) => Some(data.resources.instructions),
            _ => None,
        },
        _ => panic!("expected a V1 transaction envelope"),
    }
}

fn envelope_from_json(stdout: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    v.get("envelope").unwrap().as_str().unwrap().to_string()
}

#[test]
fn offline_build_keeps_inclusion_only_fee_and_warns() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let out = cmd
        .args([
            "tx",
            "build",
            "--source",
            TEST_SOURCE,
            "--sequence",
            "1",
            "--contract",
            TEST_CONTRACT,
            "--function",
            "hello",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("base inclusion fee"),
        "expected an offline fee warning, got: {stderr}"
    );
    let env = envelope_from_json(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(envelope_fee(&env), 100, "offline default must stay 100");
    assert_eq!(
        envelope_instructions(&env),
        None,
        "an offline build carries no Soroban footprint at all"
    );
}

#[test]
fn explicit_fee_is_honoured_offline_without_warning() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let out = cmd
        .args([
            "tx",
            "build",
            "--source",
            TEST_SOURCE,
            "--sequence",
            "1",
            "--contract",
            TEST_CONTRACT,
            "--function",
            "hello",
            "--fee",
            "54321",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("base inclusion fee"),
        "an explicit --fee should not warn, got: {stderr}"
    );
    let env = envelope_from_json(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(envelope_fee(&env), 54_321);
}

#[test]
fn network_flags_adopt_the_simulated_resource_fee() {
    let (url, sims) = mock_rpc("100000");
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let out = cmd
        .args([
            "tx",
            "--rpc-url",
            &url,
            "--network-passphrase",
            "Test SDF Network ; September 2015",
            "build",
            "--source",
            TEST_SOURCE,
            "--sequence",
            "1",
            "--contract",
            TEST_CONTRACT,
            "--function",
            "hello",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let env = envelope_from_json(&String::from_utf8_lossy(&out.stdout));
    // 100 inclusion + 100000 resource
    assert_eq!(envelope_fee(&env), 100_100);
    assert_eq!(
        envelope_instructions(&env),
        Some(1000),
        "the simulated footprint must be adopted, not just the fee"
    );
    assert_eq!(
        sims.load(Ordering::SeqCst),
        1,
        "expected exactly one simulation"
    );
}

#[test]
fn explicit_fee_wins_over_the_network_and_skips_simulation() {
    let (url, sims) = mock_rpc("100000");
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let out = cmd
        .args([
            "tx",
            "--rpc-url",
            &url,
            "--network-passphrase",
            "Test SDF Network ; September 2015",
            "build",
            "--source",
            TEST_SOURCE,
            "--sequence",
            "1",
            "--contract",
            TEST_CONTRACT,
            "--function",
            "hello",
            "--fee",
            "777",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(out.status.success());
    let env = envelope_from_json(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(envelope_fee(&env), 777);
    assert_eq!(
        sims.load(Ordering::SeqCst),
        0,
        "an explicit --fee should not cost a network round trip"
    );
}

#[test]
fn unreachable_network_falls_back_to_offline_fee_with_warning() {
    // Naming a network that cannot be reached must not turn a build that
    // would have succeeded offline into a failure. It degrades to the offline
    // envelope and says so, rather than emitting a priced-looking one.
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let out = cmd
        .args([
            "tx",
            "--rpc-url",
            UNREACHABLE_RPC,
            "build",
            "--source",
            TEST_SOURCE,
            "--sequence",
            "42",
            "--contract",
            TEST_CONTRACT,
            "--function",
            "hello",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "the build must still produce an envelope"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("could not derive the fee from simulation"),
        "the fallback must be announced, got: {stderr}"
    );
    let env = envelope_from_json(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(
        envelope_fee(&env),
        100,
        "fallback keeps the inclusion-only fee"
    );
    assert_eq!(
        envelope_instructions(&env),
        None,
        "a failed simulation must not leave a half-adopted footprint"
    );
}
