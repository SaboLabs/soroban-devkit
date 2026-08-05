use assert_cmd::Command;

/// Minimal valid WASM binary (magic + version 1).
const MINIMAL_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

#[test]
fn test_cli_verify_missing_contract_arg() {
    // `--contract` is required by clap → failure without touching RPC.
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let assert = cmd.arg("verify").assert();
    assert.failure();
}

#[test]
fn test_cli_verify_invalid_format_arg() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CABCDEFG")
        .arg("--format")
        .arg("bogus")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("Invalid format"));
}

#[test]
fn test_cli_verify_missing_wasm_file() {
    // Missing local file must fail offline with a clear message (no RPC).
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CABCDEFG")
        .arg("--wasm")
        .arg("/nonexistent/path/contract.wasm")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("Error reading WASM"));
}

#[test]
fn test_cli_verify_invalid_wasm() {
    // Invalid local WASM must fail offline (fail-fast before RPC).
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"not a wasm file").unwrap();

    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CABCDEFG")
        .arg("--wasm")
        .arg(tmp.path())
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("not valid WASM"));
}

#[test]
fn test_cli_verify_json_format_accepted() {
    // `--format json` must be parsed; an invalid local WASM still fails
    // offline, proving the JSON path is reachable without a network.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"not a wasm file").unwrap();

    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CABCDEFG")
        .arg("--wasm")
        .arg(tmp.path())
        .arg("--format")
        .arg("json")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("not valid WASM"));
}

#[test]
fn test_cli_verify_onchain_error_path() {
    // With a valid local WASM but a bogus contract id, the command must reach
    // the RPC layer and exit non-zero (offline this surfaces as a network/
    // contract error, not a panic). Exercises the on-chain fetch + error path.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), MINIMAL_WASM).unwrap();

    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let assert = cmd
        .arg("verify")
        .arg("--contract")
        .arg("CNotARealContractId")
        .arg("--wasm")
        .arg(tmp.path())
        .assert();
    assert.failure();
}
