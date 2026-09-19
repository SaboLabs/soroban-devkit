use assert_cmd::Command;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

/// Build sdkt command with isolated identity + network stores in a single temp dir.
fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

/// Start a minimal HTTP server on localhost that responds with a fixed body
/// and status, then return the base URL.
fn mock_http_server(status: &str, body: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let response_body = body.to_string();
    let status_line = status.to_string();

    thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf);

            let header = format!(
                "{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status_line,
                response_body.len(),
                response_body
            );
            let _ = sock.write_all(header.as_bytes());
        }
    });

    // Brief sleep to ensure the server thread is ready
    thread::sleep(Duration::from_millis(50));

    url
}

#[test]
fn fund_help_exposes_command() {
    let dir = tempdir().unwrap();

    sdkt_isolated(dir.path())
        .args(["identity", "fund", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Fund an identity"))
        .stdout(predicates::str::contains("--network-profile"))
        .stdout(predicates::str::contains("--format"));
}

#[test]
fn fund_missing_identity_fails() {
    let dir = tempdir().unwrap();

    // Add a network profile with friendbot URL
    sdkt_isolated(dir.path())
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
        ])
        .assert()
        .success();

    sdkt_isolated(dir.path())
        .args(["identity", "fund", "ghost", "--network-profile", "testnet"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("not found"));
}

#[test]
fn fund_missing_friendbot_url_fails() {
    let dir = tempdir().unwrap();

    // Generate an identity
    sdkt_isolated(dir.path())
        .args(["identity", "generate", "bob"])
        .assert()
        .success();

    // Add a network profile WITHOUT friendbot URL
    sdkt_isolated(dir.path())
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

    sdkt_isolated(dir.path())
        .args(["identity", "fund", "bob", "--network-profile", "mainnet"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no Friendbot URL"));
}

#[test]
fn fund_missing_profile_fails() {
    let dir = tempdir().unwrap();

    // Generate an identity
    sdkt_isolated(dir.path())
        .args(["identity", "generate", "carol"])
        .assert()
        .success();

    // Attempt to fund using a non-existent profile — must fail before any network call
    sdkt_isolated(dir.path())
        .args([
            "identity",
            "fund",
            "carol",
            "--network-profile",
            "ghost-profile",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("not found"));
}

#[test]
fn fund_success_pretty_output() {
    let dir = tempdir().unwrap();

    // Generate an identity
    sdkt_isolated(dir.path())
        .args(["identity", "generate", "dave"])
        .assert()
        .success();

    // Start a mock friendbot server returning a successful response
    let friendbot_url = mock_http_server(
        "HTTP/1.1 200 OK\r\n",
        r#"{"hash":"abc123def456","ledger":12345}"#,
    );

    // Add network profile with the mock friendbot URL
    sdkt_isolated(dir.path())
        .args([
            "network",
            "add",
            "mocknet",
            "--rpc-url",
            "https://soroban-testnet.stellar.org",
            "--passphrase",
            "Test SDF Network ; September 2015",
            "--friendbot",
            &friendbot_url,
        ])
        .assert()
        .success();

    // Run fund command and verify pretty output
    sdkt_isolated(dir.path())
        .args(["identity", "fund", "dave", "--network-profile", "mocknet"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Identity Funded via Friendbot"))
        .stdout(predicates::str::contains("dave"))
        .stdout(predicates::str::contains("mocknet"));
}

#[test]
fn fund_success_json_output() {
    let dir = tempdir().unwrap();

    // Generate an identity
    sdkt_isolated(dir.path())
        .args(["identity", "generate", "eve"])
        .assert()
        .success();

    // Start a mock friendbot server returning a successful response
    let friendbot_url =
        mock_http_server("HTTP/1.1 200 OK\r\n", r#"{"hash":"xyz789","ledger":67890}"#);

    // Add network profile with mock friendbot URL
    sdkt_isolated(dir.path())
        .args([
            "network",
            "add",
            "mocknet",
            "--rpc-url",
            "https://soroban-testnet.stellar.org",
            "--passphrase",
            "Test SDF Network ; September 2015",
            "--friendbot",
            &friendbot_url,
        ])
        .assert()
        .success();

    // Run fund command with JSON output and capture the output
    let output = sdkt_isolated(dir.path())
        .args([
            "identity",
            "fund",
            "eve",
            "--network-profile",
            "mocknet",
            "--format",
            "json",
        ])
        .output()
        .expect("Failed to execute fund command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "Command failed: stdout={} stderr={}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify valid JSON
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Output is not valid JSON: {}\nOutput was: {}", e, stdout));

    // Verify expected fields
    assert!(
        parsed.get("address").is_some(),
        "Missing 'address' field in JSON output"
    );
    assert!(
        parsed.get("status").is_some(),
        "Missing 'status' field in JSON output"
    );
}

#[test]
fn fund_http_500_error_propagates() {
    let dir = tempdir().unwrap();

    // Generate an identity
    sdkt_isolated(dir.path())
        .args(["identity", "generate", "frank"])
        .assert()
        .success();

    // Start a mock friendbot server returning 500
    let friendbot_url = mock_http_server(
        "HTTP/1.1 500 Internal Server Error\r\n",
        r#"{"error":"internal server error"}"#,
    );

    // Add network profile with the mock friendbot URL
    sdkt_isolated(dir.path())
        .args([
            "network",
            "add",
            "mocknet",
            "--rpc-url",
            "https://soroban-testnet.stellar.org",
            "--passphrase",
            "Test SDF Network ; September 2015",
            "--friendbot",
            &friendbot_url,
        ])
        .assert()
        .success();

    // Run fund command — should fail with HTTP error
    sdkt_isolated(dir.path())
        .args(["identity", "fund", "frank", "--network-profile", "mocknet"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("500"));
}

#[test]
fn fund_http_429_rate_limit() {
    let dir = tempdir().unwrap();

    // Generate an identity
    sdkt_isolated(dir.path())
        .args(["identity", "generate", "grace"])
        .assert()
        .success();

    // Start a mock friendbot server returning 429
    let friendbot_url = mock_http_server(
        "HTTP/1.1 429 Too Many Requests\r\n",
        r#"{"error":"rate limit exceeded"}"#,
    );

    // Add network profile with the mock friendbot URL
    sdkt_isolated(dir.path())
        .args([
            "network",
            "add",
            "mocknet",
            "--rpc-url",
            "https://soroban-testnet.stellar.org",
            "--passphrase",
            "Test SDF Network ; September 2015",
            "--friendbot",
            &friendbot_url,
        ])
        .assert()
        .success();

    // Run fund command — should fail with rate limit error
    sdkt_isolated(dir.path())
        .args(["identity", "fund", "grace", "--network-profile", "mocknet"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("rate limit"));
}
