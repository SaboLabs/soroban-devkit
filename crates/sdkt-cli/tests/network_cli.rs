//! Integration tests for `sdkt network` and network-profile resolution.
//!
//! Every test here is CI-safe:
//! - No test depends on a locally running RPC server.
//! - No test depends on internet access.
//! - No test depends on machine-specific configuration.
//!
//! The pure precedence logic (flags > profile > .sdkt.toml > defaults) is
//! covered by unit tests in `main.rs` (`resolver_tests`), which need no I/O.
//! These integration tests cover the CLI surface and the one deterministic
//! end-to-end path: a missing profile is rejected *before* any network call.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

/// Build a `sdkt` command with `SDKT_NETWORK_DIR` pointing at `dir`.
fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_NETWORK_DIR", dir);
    cmd
}

// ---------- : `sdkt network` management ----------

#[test]
fn network_add_then_list_pretty() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "testnet",
            "--rpc-url",
            "https://soroban-testnet.stellar.org",
            "--passphrase",
            "Test SDF Network ; September 2015",
            "--friendbot",
            "https://friendbot.stellar.org",
            "--description",
            "Stellar testnet",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Network profile 'testnet' saved."));

    sdkt(dir.path())
        .args(["network", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("testnet"))
        .stdout(predicate::str::contains(
            "https://soroban-testnet.stellar.org",
        ));
}

#[test]
fn network_add_then_show_json() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "mainnet",
            "--rpc-url",
            "https://soroban-mainnet.stellar.org",
            "--passphrase",
            "Public Global Stellar Network ; September 2015",
        ])
        .assert()
        .success();

    sdkt(dir.path())
        .args(["network", "show", "mainnet", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"mainnet\""))
        .stdout(predicate::str::contains(
            "Public Global Stellar Network ; September 2015",
        ));
}

#[test]
fn network_show_missing_errors() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["network", "show", "ghost"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not found").not())
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn network_remove_deletes_profile() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "solo",
            "--rpc-url",
            "https://solo.example",
            "--passphrase",
            "Solo Passphrase",
        ])
        .assert()
        .success();

    assert!(dir.path().join("solo.json").exists());

    sdkt(dir.path())
        .args(["network", "remove", "solo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Network profile 'solo' removed."));

    assert!(!dir.path().join("solo.json").exists());

    sdkt(dir.path())
        .args(["network", "remove", "solo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn network_add_overwrites_existing() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "net",
            "--rpc-url",
            "https://old.example",
            "--passphrase",
            "Old",
        ])
        .assert()
        .success();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "net",
            "--rpc-url",
            "https://new.example",
            "--passphrase",
            "New",
        ])
        .assert()
        .success();

    let count = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .path()
                .extension()
                .and_then(|s| s.to_str())
                == Some("json")
        })
        .count();
    assert_eq!(count, 1);

    sdkt(dir.path())
        .args(["network", "show", "net", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("https://new.example"));
}

#[test]
fn network_list_empty_message() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["network", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No network profiles found."));
}

// ---------- : network-profile resolution (CLI surface, CI-safe) ----------

#[test]
fn network_profile_not_found_fails_before_rpc() {
    let dir = tempdir().unwrap();

    // A profile that does not exist must be rejected at resolution time,
    // before any RPC call is attempted. This is deterministic: it touches only
    // the local network store (via SDKT_NETWORK_DIR) and never reaches the network.
    sdkt(dir.path())
        .args(["account", "GABC", "--network-profile", "ghost"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn rpc_commands_expose_network_profile_flag() {
    // Backward compatibility: the flag is present on RPC commands, and the
    // help output still parses (existing interface unchanged). No network used.
    for cmd in ["inspect", "account", "events", "health", "verify", "deploy"] {
        sdkt(std::path::Path::new("/dev/null"))
            .args([cmd, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--network-profile"));
    }
}

#[test]
fn existing_commands_work_without_profiles() {
    let dir = tempdir().unwrap();

    // Commands without --network-profile must still parse and behave exactly
    // as before. `sdkt build --help` is a non-RPC command that must succeed
    // offline; `sdkt network list` is the profile manager itself.
    sdkt(dir.path())
        .args(["build", "--help"])
        .assert()
        .success();

    sdkt(dir.path())
        .args(["network", "list"])
        .assert()
        .success();
}

// ---------- `sdkt network check` (reachability) ----------

/// Return an `http://127.0.0.1:<port>` URL that is guaranteed to refuse
/// connections: the listener is bound to obtain a free port, then dropped so
/// nothing is listening when the CLI probes it.
fn refused_local_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{}", port)
}

/// Read one HTTP/1.1 request (headers + body) from `stream`.
fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;

    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut header_end: Option<usize> = None;
    let mut content_length = 0usize;

    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }

        if header_end.is_none() {
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                header_end = Some(pos + 4);
                let headers = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                content_length = headers
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
            }
        }

        if let Some(end) = header_end {
            if buf.len() >= end + content_length {
                break;
            }
        }
    }

    String::from_utf8_lossy(&buf).to_string()
}

/// Spawn a minimal JSON-RPC mock that answers `getLatestLedger` and
/// `getHealth` on `127.0.0.1`. The returned URL is safe to use from the CLI
/// child process. The server thread is detached and lives for the test
/// process; it never touches the public internet.
fn spawn_mock_rpc(sequence: u32, protocol_version: u32) -> String {
    use std::io::Write;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock RPC");
    let addr = listener.local_addr().unwrap();

    std::thread::spawn(move || loop {
        let (mut stream, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(_) => return,
        };

        let request = read_http_request(&mut stream);
        let body = if request.contains("getHealth") {
            r#"{"jsonrpc":"2.0","id":1,"result":{"status":"healthy"}}"#.to_string()
        } else {
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"id":"ledger","protocolVersion":{},"sequence":{}}}}}"#,
                protocol_version, sequence
            )
        };

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });

    format!("http://{}", addr)
}

#[test]
fn network_check_missing_profile_fails() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["network", "check", "ghost"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn network_check_unreachable_profile_fails() {
    let dir = tempdir().unwrap();
    let url = refused_local_url();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "dead",
            "--rpc-url",
            &url,
            "--passphrase",
            "Dead Network",
        ])
        .assert()
        .success();

    // Non-zero exit with an actionable message that names the URL tried.
    sdkt(dir.path())
        .args(["network", "check", "dead"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("is NOT reachable"))
        .stdout(predicate::str::contains(&url))
        .stdout(predicate::str::contains("unreachable"));
}

#[test]
fn network_check_unreachable_json_shape() {
    let dir = tempdir().unwrap();
    let url = refused_local_url();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "dead",
            "--rpc-url",
            &url,
            "--passphrase",
            "Dead Network",
        ])
        .assert()
        .success();

    sdkt(dir.path())
        .args(["network", "check", "dead", "--format", "json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"reachable\":false"))
        .stdout(predicate::str::contains("\"status\":null"))
        .stdout(predicate::str::contains("\"latest_ledger\":null"))
        .stdout(predicate::str::contains("\"protocol_version\":null"))
        .stdout(predicate::str::contains("\"error\":"));
}

#[test]
fn network_check_healthy_mock_rpc_succeeds() {
    let dir = tempdir().unwrap();
    let url = spawn_mock_rpc(4242, 21);

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "mock",
            "--rpc-url",
            &url,
            "--passphrase",
            "Mock Network",
        ])
        .assert()
        .success();

    sdkt(dir.path())
        .args(["network", "check", "mock"])
        .assert()
        .success()
        .stdout(predicate::str::contains("is reachable"))
        .stdout(predicate::str::contains("Latest ledger:    4242"))
        .stdout(predicate::str::contains("Protocol version: 21"));
}

#[test]
fn network_check_healthy_mock_rpc_json_shape() {
    let dir = tempdir().unwrap();
    let url = spawn_mock_rpc(99, 20);

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "mock",
            "--rpc-url",
            &url,
            "--passphrase",
            "Mock Network",
        ])
        .assert()
        .success();

    sdkt(dir.path())
        .args(["network", "check", "mock", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"reachable\":true"))
        .stdout(predicate::str::contains("\"status\":\"healthy\""))
        .stdout(predicate::str::contains("\"latest_ledger\":99"))
        .stdout(predicate::str::contains("\"protocol_version\":20"))
        .stdout(predicate::str::contains("\"error\":null"));
}

#[test]
fn network_check_does_not_mutate_stored_profile() {
    let dir = tempdir().unwrap();
    let url = refused_local_url();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "stable",
            "--rpc-url",
            &url,
            "--passphrase",
            "Stable Network",
        ])
        .assert()
        .success();

    let path = dir.path().join("stable.json");
    let before = std::fs::read_to_string(&path).unwrap();

    // The check may fail (the endpoint is unreachable) but must not rewrite
    // the on-disk profile.
    sdkt(dir.path())
        .args(["network", "check", "stable"])
        .assert()
        .failure();

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(before, after, "network check must not mutate the profile");
}
