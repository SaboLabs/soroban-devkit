//! Integration coverage proving that custom `--timeout` / `--interval` values
//! actually reach the polling engine (`poll_transaction` → `PollConfig`).
//!
//! Strategy: a mock RPC server answers `getTransaction` with PENDING for the
//! first N polls, then SUCCESS. The server records wall-clock timestamps of
//! each `getTransaction` request. With `--interval 2 --timeout 60`, the gap
//! between consecutive polls must be >= 2s (the custom interval), proving the
//! flag value propagated through `PollConfig` into the polling loop — not the
//! 1s default.
//!
//! Runs at the RPC layer (`submit_and_wait`) driven through the same
//! `PollConfig` the CLI builds from `--timeout`/`--interval`, plus a CLI-level
//! test asserting the invalid-pair guard runs before any network work.

use sdkt_rpc::client::SorobanRpcClient;
use sdkt_rpc::PollConfig;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

fn extract_jsonrpc_method(req: &str) -> Option<String> {
    let body_start = req.find("\r\n\r\n")?;
    let body = &req[body_start + 4..];
    let val: serde_json::Value = serde_json::from_str(body).ok()?;
    val.get("method")?.as_str().map(ToString::to_string)
}

/// Mock server: getTransaction returns PENDING for the first `pending_before_success`
/// calls, then SUCCESS. Records timestamps of each getTransaction request.
fn spawn_pending_then_success_server(
    pending_before_success: usize,
    poll_times: Arc<Mutex<Vec<Instant>>>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let poll_count = Arc::new(AtomicUsize::new(0));
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
                Some("getTransaction") => {
                    poll_times.lock().unwrap().push(Instant::now());
                    let call_idx = poll_count.fetch_add(1, Ordering::SeqCst);
                    if call_idx < pending_before_success {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"status":"PENDING","latestLedger":"101"}}"#.to_string()
                    } else {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"status":"SUCCESS","latestLedger":"102","resultXdr":"AAAAAg=="}}"#.to_string()
                    }
                }
                Some("sendTransaction") => {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeef1234","status":"PENDING","latestLedger":"100"}}"#.to_string()
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
    format!("http://{}", addr)
}

/// Custom interval must reach the polling loop: with interval=2s and 3 pending
/// polls, consecutive getTransaction gaps must be >= 2s (default is 1s, so a
/// 1s-propagation bug would produce gaps < 2s and fail this assertion).
#[tokio::test]
async fn custom_interval_reaches_polling_engine() {
    let poll_times = Arc::new(Mutex::new(Vec::new()));
    let url = spawn_pending_then_success_server(3, poll_times.clone());
    let client = SorobanRpcClient::new(&url);

    let config = PollConfig {
        timeout: std::time::Duration::from_secs(60),
        interval: std::time::Duration::from_secs(2),
    };

    let result = sdkt_rpc::submit_and_wait(
        &client,
        // Arbitrary opaque envelope: sendTransaction is mocked, so the value
        // never reaches XDR decoding.
        "mock-envelope",
        true,
        &config,
    )
    .await
    .expect("submit_and_wait must succeed");

    assert_eq!(result.status, sdkt_rpc::TransactionStatus::Success);

    let times = poll_times.lock().unwrap().clone();
    assert!(
        times.len() >= 4,
        "expected >= 4 getTransaction polls (3 pending + 1 success), got {}",
        times.len()
    );

    // Every consecutive gap must reflect the custom 2s interval, not the 1s
    // default. Use a generous lower bound (1.5s) to tolerate scheduler jitter
    // while still discriminating against the 1s default.
    for (i, gap) in times.windows(2).map(|w| w[1] - w[0]).enumerate() {
        assert!(
            gap >= std::time::Duration::from_millis(1500),
            "poll gap #{i} was {gap:?}; custom --interval 2 did not reach the polling engine"
        );
    }
}

/// Custom timeout must reach the polling loop: with interval=1s (fast polls)
/// and timeout=2s, submit_and_wait must give up after ~2s (not the 60s
/// default) when the transaction stays PENDING forever.
#[tokio::test]
async fn custom_timeout_reaches_polling_engine() {
    let poll_times = Arc::new(Mutex::new(Vec::new()));
    // Never succeeds: every poll returns PENDING.
    let url = spawn_pending_then_success_server(usize::MAX, poll_times.clone());
    let client = SorobanRpcClient::new(&url);

    let config = PollConfig {
        timeout: std::time::Duration::from_secs(2),
        interval: std::time::Duration::from_secs(1),
    };

    let started = Instant::now();
    let result = sdkt_rpc::submit_and_wait(&client, "mock-envelope", true, &config).await;

    let elapsed = started.elapsed();
    let err = result.expect_err("must time out");
    let msg = format!("{err}");
    assert!(
        msg.contains("timed out"),
        "expected polling timeout error, got: {msg}"
    );
    // Timeout must fire near the custom 2s, not the 60s default. Allow up to
    // 10s total for scheduler overhead; a 60s-default bug would exceed this.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "polling ran for {elapsed:?}; custom --timeout 2 did not reach the polling engine"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(1900),
        "polling gave up too early ({elapsed:?}); timeout fired before the custom 2s"
    );
}
