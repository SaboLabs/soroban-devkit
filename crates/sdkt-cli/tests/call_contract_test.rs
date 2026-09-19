use assert_cmd::Command;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

fn sdkt_isolated(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

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

    thread::sleep(Duration::from_millis(50));
    url
}

const VALID_CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const VALID_ADDRESS: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

// JSON-RPC 2.0 responses with escaped quotes for use in Rust strings
const MOCK_SIM_OK: &str =
    "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"results\":[{\"xdr\":\"AAAAAQ==\",\"auth\":[]}],\"latestLedger\":\"12345\",\"events\":[]}}";
const MOCK_SIM_ERR: &str =
    "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"error\":\"ContractError(1)\",\"latestLedger\":\"12345\",\"events\":[]}}";
const MOCK_500: &str = "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":\"internal\"}";

fn add_mock_profile(dir: &std::path::Path, rpc_url: &str) {
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
fn call_help_exposes_command() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args(["call", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Invoke a contract"))
        .stdout(predicates::str::contains("--network-profile"))
        .stdout(predicates::str::contains("--args"))
        .stdout(predicates::str::contains("--format"));
}

#[test]
fn call_help_mentions_typed_args() {
    let dir = tempdir().unwrap();
    let output = sdkt_isolated(dir.path())
        .args(["call", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("TYPE:VALUE") || stdout.contains("u32") || stdout.contains("address"),
        "Help should mention typed arg format, got: {}",
        stdout
    );
}

#[test]
fn call_missing_function_errors() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args(["call", VALID_CONTRACT])
        .assert()
        .failure()
        .stderr(predicates::str::contains("FUNCTION"));
}

#[test]
fn call_invalid_arg_format_no_colon() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "balance",
            "--args",
            "invalid_no_colon",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("TYPE:VALUE"));
}

#[test]
fn call_invalid_arg_type() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args(["call", VALID_CONTRACT, "balance", "--args", "u99:100"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("unknown arg type"));
}

#[test]
fn call_invalid_u32_value() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "set_value",
            "--args",
            "u32:not_a_number",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("invalid u32"));
}

#[test]
fn call_invalid_bool_value() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args(["call", VALID_CONTRACT, "toggle", "--args", "bool:yes"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("invalid bool"));
}

#[test]
fn call_invalid_address_value() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "transfer",
            "--args",
            "address:INVALID_KEY",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("invalid Stellar address"));
}

#[test]
fn call_success_pretty_output() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_OK);
    add_mock_profile(dir.path(), &rpc_url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "balance",
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Contract:"))
        .stdout(predicates::str::contains("Function:"))
        .stdout(predicates::str::contains("Result:"));
}

#[test]
fn call_success_json_output() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_OK);
    add_mock_profile(dir.path(), &rpc_url);

    let output = sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "balance",
            "--network-profile",
            "mocknet",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "Failed: stdout={} stderr={}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {}\nOutput: {}", e, stdout));

    assert!(parsed.get("contract").is_some(), "Missing contract");
    assert!(parsed.get("function").is_some(), "Missing function");
    assert!(parsed.get("result").is_some(), "Missing result");
    assert!(parsed.get("events").is_some(), "Missing events");
}

#[test]
fn call_with_typed_args_u32() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_OK);
    add_mock_profile(dir.path(), &rpc_url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "set_value",
            "--network-profile",
            "mocknet",
            "--args",
            "u32:42",
        ])
        .assert()
        .success();
}

#[test]
fn call_with_typed_args_address() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_OK);
    add_mock_profile(dir.path(), &rpc_url);

    let arg = format!("address:{VALID_ADDRESS}");
    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "transfer",
            "--network-profile",
            "mocknet",
            "--args",
            &arg,
        ])
        .assert()
        .success();
}

#[test]
fn call_with_typed_args_string() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_OK);
    add_mock_profile(dir.path(), &rpc_url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "set_name",
            "--network-profile",
            "mocknet",
            "--args",
            "string:hello_world",
        ])
        .assert()
        .success();
}

#[test]
fn call_with_typed_args_bool() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_OK);
    add_mock_profile(dir.path(), &rpc_url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "toggle",
            "--network-profile",
            "mocknet",
            "--args",
            "bool:true",
        ])
        .assert()
        .success();
}

#[test]
fn call_with_no_args() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_OK);
    add_mock_profile(dir.path(), &rpc_url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "get_total",
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .success();
}

#[test]
fn call_simulation_error() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_ERR);
    add_mock_profile(dir.path(), &rpc_url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "fails",
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("simulation error"));
}

#[test]
fn call_rpc_server_error() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 500 Internal Server Error\r\n", MOCK_500);
    add_mock_profile(dir.path(), &rpc_url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "balance",
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("RPC simulation failed"));
}

#[test]
fn call_missing_profile() {
    let dir = tempdir().unwrap();
    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "balance",
            "--network-profile",
            "ghost-profile",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("not found"));
}

#[test]
fn call_does_not_require_identity_store() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_OK);
    add_mock_profile(dir.path(), &rpc_url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "balance",
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .success();
}

#[test]
fn call_does_not_modify_identity_store() {
    let dir = tempdir().unwrap();
    let rpc_url = mock_http_server("HTTP/1.1 200 OK\r\n", MOCK_SIM_OK);
    add_mock_profile(dir.path(), &rpc_url);

    sdkt_isolated(dir.path())
        .args([
            "call",
            VALID_CONTRACT,
            "balance",
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .success();

    let identity_dir = dir.path().join("identity");
    if identity_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&identity_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "call should not modify identity store: {:?}",
            entries
        );
    }
}
