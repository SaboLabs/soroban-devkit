//! Integration tests for `sdkt project deploy` with per-contract constructor
//! arguments (`ctor_args` field in `.sdkt.toml`).
//!
//! Validates that:
//! - Constructor arguments declared in config are forwarded to each contract
//! - Empty ctor_args deploys byte-identically to pre-feature contracts
//! - Invalid typed args fail before WASM upload (fail-fast validation)
//! - Args are validated per-contract with clear error messages

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use stellar_xdr::{Limited, Limits, ReadXdr, TransactionEnvelope};
use tempfile::TempDir;

/// A real contractspecv0 WASM fixture.
static CONTRACT_WASM: &[u8] = include_bytes!("fixtures/us_new.wasm");

/// LedgerEntry XDR for an account with seq_num 41 (so next sequence = 42).
const ACCOUNT_ENTRY_XDR: &str =
    "AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAO5rKAAAAAAAAAAApAAAAAAAAAAAAAAAAAAAAAAEBAQEAAAAAAAAAAAAAAAA=";

/// SorobanTransactionData XDR: empty footprint, 1000 instructions, 150 stroops
/// resource fee.
const SOROBAN_DATA_XDR: &str = "AAAAAAAAAAAAAAAAAAAD6AAAAAoAAAAKAAAAAAAAAJY=";

fn sdkt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

/// Mock RPC server for project deploy tests
struct MockServer {
    url: String,
    submitted_envelopes: Arc<std::sync::Mutex<Vec<String>>>,
    rpc_calls: Arc<AtomicUsize>,
}

impl MockServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let submitted = Arc::new(std::sync::Mutex::new(Vec::new()));
        let submitted_thread = submitted.clone();
        let rpc_calls = Arc::new(AtomicUsize::new(0));
        let rpc_calls_thread = rpc_calls.clone();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let mut sock = match conn {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let mut buf = [0u8; 16384];
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    continue;
                }
                rpc_calls_thread.fetch_add(1, Ordering::SeqCst);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let method = extract_jsonrpc_method(&req);

                // Capture sendTransaction envelopes for arg verification
                if method.as_deref() == Some("sendTransaction") {
                    if let Some(body_start) = req.find("\r\n\r\n") {
                        let body = &req[body_start + 4..];
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) {
                            if let Some(xdr_b64) = parsed["params"]["transaction"].as_str() {
                                submitted_thread.lock().unwrap().push(xdr_b64.to_string());
                            }
                        }
                    }
                }

                let body = match method.as_deref() {
                    Some("getLedgerEntries") => {
                        format!(
                            r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"AAAAAA==","xdr":"{ACCOUNT_ENTRY_XDR}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                        )
                    }
                    Some("simulateTransaction") => format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"transactionData":"{SOROBAN_DATA_XDR}","minResourceFee":"150","results":[{{"xdr":"AAAAAQ==","auth":[]}}],"latestLedger":"100","events":[]}}}}"#
                    ),
                    Some("sendTransaction") => {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"hash":"deadbeefcafe","status":"PENDING","latestLedger":"100"}}"#
                            .to_string()
                    }
                    Some("getTransaction") => {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"status":"SUCCESS","latestLedger":"101","resultXdr":"AAAAAg=="}}"#
                            .to_string()
                    }
                    _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#.to_string(),
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        Self {
            url,
            submitted_envelopes: submitted,
            rpc_calls,
        }
    }

    fn envelopes(&self) -> Vec<String> {
        self.submitted_envelopes.lock().unwrap().clone()
    }

    /// Total number of JSON-RPC requests received (any method).
    fn rpc_calls(&self) -> usize {
        self.rpc_calls.load(Ordering::SeqCst)
    }
}

fn extract_jsonrpc_method(req: &str) -> Option<String> {
    let body_start = req.find("\r\n\r\n")?;
    let body = &req[body_start + 4..];
    let val: serde_json::Value = serde_json::from_str(body).ok()?;
    val.get("method")?.as_str().map(ToString::to_string)
}

fn setup_project_with_args(dir: &Path) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"
ctor_args = ["address:GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF", "u32:1000000"]

[contracts.vault]
path = "contracts/vault"
depends_on = ["token"]
"#;
    std::fs::write(dir.join(".sdkt.toml"), manifest).unwrap();

    for alias in ["token", "vault"] {
        let target = dir
            .join("contracts")
            .join(alias)
            .join("target")
            .join(sdkt_core::build::WASM_BUILD_TARGET)
            .join("release");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join(format!("{alias}.wasm")), CONTRACT_WASM).unwrap();
    }
}

fn setup_project_invalid_args(dir: &Path) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"
ctor_args = ["u32:not_a_number"]
"#;
    std::fs::write(dir.join(".sdkt.toml"), manifest).unwrap();

    let target = dir
        .join("contracts")
        .join("token")
        .join("target")
        .join(sdkt_core::build::WASM_BUILD_TARGET)
        .join("release");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("token.wasm"), CONTRACT_WASM).unwrap();
}

fn setup_project_no_args(dir: &Path) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"
"#;
    std::fs::write(dir.join(".sdkt.toml"), manifest).unwrap();

    let target = dir
        .join("contracts")
        .join("token")
        .join("target")
        .join(sdkt_core::build::WASM_BUILD_TARGET)
        .join("release");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("token.wasm"), CONTRACT_WASM).unwrap();
}

fn generate_default_identity(dir: &Path) {
    sdkt(dir)
        .args(["identity", "generate", "alice"])
        .assert()
        .success();
    sdkt(dir)
        .args(["identity", "default", "alice"])
        .assert()
        .success();
}

fn deploy_args(mock: &MockServer) -> Vec<String> {
    vec![
        "project".into(),
        "--rpc-url".into(),
        mock.url.clone(),
        "--network-passphrase".into(),
        "Test SDF Network ; September 2015".into(),
        "deploy".into(),
    ]
}

/// A contract with ctor_args deploys with those arguments forwarded to the
/// create transaction. Verify by decoding the CreateContractV2 operation and
/// checking the constructor args contain the configured address and u32.
#[test]
fn project_deploy_forwards_constructor_args() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project_with_args(project_dir);
    generate_default_identity(project_dir);

    let mock = MockServer::start();
    sdkt(project_dir)
        .current_dir(project_dir)
        .args(deploy_args(&mock))
        .assert()
        .success()
        .stdout(predicate::str::contains("Deploying alias 'token'"))
        .stdout(predicate::str::contains("Deploying alias 'vault'"));

    // token has 2 args; vault has none. Expect 4 sendTransaction calls total
    // (upload + create for each contract).
    let envelopes = mock.envelopes();
    assert_eq!(
        envelopes.len(),
        4,
        "expected 4 transactions (upload+create per contract)"
    );

    // The second envelope is token's create transaction with constructor args.
    // Parse it and verify the InvokeHostFunction operation contains the
    // configured address and u32:1000000.
    let token_create_xdr = &envelopes[1];
    let raw = STANDARD.decode(token_create_xdr).unwrap();
    let mut cursor = std::io::Cursor::new(&raw);
    let mut limited = Limited::new(&mut cursor, Limits::none());
    let envelope = TransactionEnvelope::read_xdr(&mut limited).unwrap();

    // Extract operations from the transaction
    let ops = match &envelope {
        TransactionEnvelope::Tx(tx) => &tx.tx.operations,
        TransactionEnvelope::TxV0(tx) => &tx.tx.operations,
        TransactionEnvelope::TxFeeBump(_) => panic!("unexpected fee bump envelope"),
    };

    assert!(
        !ops.is_empty(),
        "token create envelope must have at least one operation"
    );

    // The operation should be InvokeHostFunction with CreateContractV2
    use sdkt_xdr::IntoScVal;
    use stellar_xdr::{HostFunction, OperationBody, ScVal};

    let op_body = &ops[0].body;
    let host_fn = match op_body {
        OperationBody::InvokeHostFunction(invoke) => &invoke.host_function,
        _ => panic!("expected InvokeHostFunction operation, got: {:?}", op_body),
    };

    match host_fn {
        HostFunction::CreateContractV2(args) => {
            assert_eq!(
                args.constructor_args.len(),
                2,
                "token create must carry exactly the two configured ctor_args"
            );

            // ctor_args = ["address:GAAAA...WHF", "u32:1000000"] — the address
            // is the all-zero account (32 zero bytes).
            let expected_address = {
                let addr = sdkt_xdr::Address::from_strkey(
                    "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
                )
                .expect("valid account strkey");
                addr.into_scval().expect("address to ScVal")
            };
            assert_eq!(
                args.constructor_args[0], expected_address,
                "first constructor arg must be the configured address"
            );
            assert_eq!(
                args.constructor_args[1],
                ScVal::U32(1_000_000),
                "second constructor arg must be the configured u32:1000000"
            );
        }
        HostFunction::CreateContract(_) => {
            panic!("expected CreateContractV2 (with args), got CreateContract (no args)")
        }
        _ => panic!("expected CreateContract operation, got: {:?}", host_fn),
    }

    // vault (4th envelope) has no ctor_args, so it deploys via the V1
    // CreateContract path with no constructor arguments at all.
    let vault_create_xdr = &envelopes[3];
    let raw = STANDARD.decode(vault_create_xdr).unwrap();
    let mut cursor = std::io::Cursor::new(&raw);
    let mut limited = Limited::new(&mut cursor, Limits::none());
    let vault_envelope = TransactionEnvelope::read_xdr(&mut limited).unwrap();

    let vault_ops = match &vault_envelope {
        TransactionEnvelope::Tx(tx) => &tx.tx.operations,
        TransactionEnvelope::TxV0(tx) => &tx.tx.operations,
        TransactionEnvelope::TxFeeBump(_) => panic!("unexpected fee bump envelope"),
    };

    assert!(
        !vault_ops.is_empty(),
        "vault create envelope must have at least one operation"
    );

    let vault_host_fn = match &vault_ops[0].body {
        OperationBody::InvokeHostFunction(invoke) => &invoke.host_function,
        body => panic!("expected InvokeHostFunction operation, got: {:?}", body),
    };
    match vault_host_fn {
        HostFunction::CreateContract(_) => {}
        HostFunction::CreateContractV2(_) => panic!(
            "vault has no ctor_args and must deploy via CreateContract (V1), not CreateContractV2"
        ),
        other => panic!("expected CreateContract operation, got: {:?}", other),
    }
}

/// Invalid constructor arguments must fail before WASM upload, with the same
/// error message as `sdkt deploy --arg` (reusing parse_scval_args validation).
#[test]
fn project_deploy_invalid_args_fail_fast_before_upload() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project_invalid_args(project_dir);
    generate_default_identity(project_dir);

    let mock = MockServer::start();
    let assert = sdkt(project_dir)
        .current_dir(project_dir)
        .args(deploy_args(&mock))
        .assert()
        .failure();

    // The complete, user-facing error must name the contract and carry the
    // exact same parse error as the single-contract `sdkt deploy --arg` path.
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr
            .contains("Invalid constructor argument for 'token': invalid u32 value: not_a_number"),
        "unexpected project deploy error:\n{stderr}"
    );

    // Cross-check that the underlying message is byte-for-byte what
    // `sdkt deploy --arg u32:not_a_number` reports (offline; parsing happens
    // before any WASM read, identity lookup, or network I/O).
    let single_dir = TempDir::new().unwrap();
    let wasm_file = single_dir.path().join("test.wasm");
    std::fs::write(&wasm_file, CONTRACT_WASM).unwrap();
    let single = sdkt(single_dir.path())
        .args([
            "deploy",
            "--wasm",
            wasm_file.to_str().unwrap(),
            "--identity",
            "default",
            "--arg",
            "u32:not_a_number",
        ])
        .assert()
        .failure();
    let single_stderr = String::from_utf8_lossy(&single.get_output().stderr).to_string();
    assert!(
        single_stderr.contains("invalid u32 value: not_a_number"),
        "sdkt deploy --arg must report the same parse error:\n{single_stderr}"
    );
    assert!(
        stderr.contains("invalid u32 value: not_a_number"),
        "project deploy must reuse the sdkt deploy --arg parse error"
    );

    // Fail-fast: validation must short-circuit before upload or submission, so
    // the mock RPC server must never be contacted at all.
    let envelopes = mock.envelopes();
    assert_eq!(
        envelopes.len(),
        0,
        "no transactions should be sent when args are invalid"
    );
    assert_eq!(
        mock.rpc_calls(),
        0,
        "no RPC call (upload, simulate, or submit) may occur when args are invalid"
    );
}

/// A contract without ctor_args deploys with an empty constructor args vector,
/// maintaining byte-identical behavior with pre-feature deployments.
#[test]
fn project_deploy_no_args_unchanged_behavior() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project_no_args(project_dir);
    generate_default_identity(project_dir);

    let mock = MockServer::start();
    sdkt(project_dir)
        .current_dir(project_dir)
        .args(deploy_args(&mock))
        .assert()
        .success()
        .stdout(predicate::str::contains("Deploying alias 'token'"));

    // Expect 2 sendTransaction calls (upload + create).
    let envelopes = mock.envelopes();
    assert_eq!(
        envelopes.len(),
        2,
        "expected 2 transactions (upload+create)"
    );

    // The create transaction must carry no constructor arguments: contracts
    // without configured ctor_args deploy via the V1 CreateContract path.
    let create_xdr = &envelopes[1];
    let raw = STANDARD.decode(create_xdr).unwrap();
    let mut cursor = std::io::Cursor::new(&raw);
    let mut limited = Limited::new(&mut cursor, Limits::none());
    let envelope = TransactionEnvelope::read_xdr(&mut limited).unwrap();

    let ops = match &envelope {
        TransactionEnvelope::Tx(tx) => &tx.tx.operations,
        TransactionEnvelope::TxV0(tx) => &tx.tx.operations,
        TransactionEnvelope::TxFeeBump(_) => panic!("unexpected fee bump envelope"),
    };
    assert!(!ops.is_empty(), "create envelope must have an operation");

    use stellar_xdr::{HostFunction, OperationBody};
    let host_fn = match &ops[0].body {
        OperationBody::InvokeHostFunction(invoke) => &invoke.host_function,
        body => panic!("expected InvokeHostFunction operation, got: {:?}", body),
    };
    match host_fn {
        HostFunction::CreateContract(_) => {}
        HostFunction::CreateContractV2(_) => panic!(
            "contract without ctor_args must deploy via CreateContract (V1), not CreateContractV2"
        ),
        other => panic!("expected CreateContract operation, got: {:?}", other),
    }
}

/// Backward-compatibility regression: the project deploy path now routes
/// through `deploy_contract_with_args` even for contracts with no configured
/// `ctor_args`. With an empty argument vector it must produce byte-for-byte the
/// same upload and create transactions as the pre-feature single-contract
/// `sdkt deploy` path (which also skips the constructor-args machinery).
///
/// Both commands are run against isolated mock RPC servers with the same fixed
/// salt, identity, network passphrase, and WASM, so any divergence in the
/// emitted transaction XDR fails the equality assertions below.
#[test]
fn project_deploy_empty_args_matches_single_deploy_byte_for_byte() {
    let project = TempDir::new().unwrap();
    let project_dir = project.path();
    setup_project_no_args(project_dir);
    generate_default_identity(project_dir);

    let wasm_path = project_dir
        .join("contracts/token/target")
        .join(sdkt_core::build::WASM_BUILD_TARGET)
        .join("release/token.wasm");
    let salt = "42".repeat(20);

    // Path 1: single-contract `sdkt deploy` with no `--arg` (legacy behavior).
    let single_mock = MockServer::start();
    sdkt(project_dir)
        .current_dir(project_dir)
        .args([
            "deploy",
            "--wasm",
            wasm_path.to_str().unwrap(),
            "--identity",
            "alice",
            "--salt",
            &salt,
            "--rpc-url",
            &single_mock.url,
            "--network-passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success();

    // Path 2: `sdkt project deploy` with an empty `ctor_args` vector.
    let project_mock = MockServer::start();
    sdkt(project_dir)
        .current_dir(project_dir)
        .args([
            "project",
            "--rpc-url",
            &project_mock.url,
            "--network-passphrase",
            "Test SDF Network ; September 2015",
            "deploy",
            "--identity",
            "alice",
            "--salt",
            &salt,
        ])
        .assert()
        .success();

    let single = single_mock.envelopes();
    let project_envs = project_mock.envelopes();
    assert_eq!(single.len(), 2, "single deploy must upload + create");
    assert_eq!(
        project_envs.len(),
        2,
        "project deploy must upload + create for a no-args contract"
    );

    // Upload (index 0) and create (index 1) envelopes must be identical:
    // adding per-contract ctor_args support must not perturb the empty path.
    assert_eq!(
        single[0], project_envs[0],
        "empty-args upload transaction must match the single deploy path"
    );
    assert_eq!(
        single[1], project_envs[1],
        "empty-args create transaction must match the single deploy path"
    );
}

/// Config parsing: ctor_args field is parsed as Vec<String>.
#[test]
fn config_parses_ctor_args_field() {
    let toml_data = r#"
[contracts.token]
path = "contracts/token"
ctor_args = ["address:G...", "u32:100"]
"#;
    let config = sdkt_core::DevKitConfig::from_toml(toml_data).unwrap();
    let token = config.contracts.get("token").unwrap();
    assert_eq!(token.ctor_args.len(), 2);
    assert_eq!(token.ctor_args[0], "address:G...");
    assert_eq!(token.ctor_args[1], "u32:100");
}

/// Omitted ctor_args field defaults to empty Vec.
#[test]
fn config_ctor_args_defaults_to_empty() {
    let toml_data = r#"
[contracts.token]
path = "contracts/token"
"#;
    let config = sdkt_core::DevKitConfig::from_toml(toml_data).unwrap();
    let token = config.contracts.get("token").unwrap();
    assert!(token.ctor_args.is_empty());
}
