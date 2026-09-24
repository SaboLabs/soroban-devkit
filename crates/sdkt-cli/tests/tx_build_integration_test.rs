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
