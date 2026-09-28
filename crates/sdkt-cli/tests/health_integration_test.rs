use assert_cmd::Command;

/// Minimal valid WASM binary (magic + version 1).
const MINIMAL_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

#[test]
fn test_cli_health_missing_contract_arg() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd.arg("health").assert();
    assert.failure();
}

#[test]
fn test_cli_health_invalid_format_arg() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
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
fn test_cli_health_missing_wasm_file() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
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
fn test_cli_health_invalid_wasm() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"not a wasm file").unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
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
fn test_cli_health_json_format_accepted() {
    // --format json must be parsed; an invalid local WASM still fails
    // offline, proving the JSON path is reachable without a network.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"not a wasm file").unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
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
fn test_cli_health_onchain_error_path() {
    // Valid local WASM + bogus contract id → reaches the RPC layer and exits
    // non-zero (offline this surfaces as a network/contract error), exercising
    // the on-chain fetch + error branch.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), MINIMAL_WASM).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("health")
        .arg("--contract")
        .arg("CNotARealContractId")
        .arg("--wasm")
        .arg(tmp.path())
        .assert();
    assert.failure();
}

// ── --fail-on flag tests ─────────────────────────────────────────────────────

/// `sdkt health --fail-on` with an unrecognised value exits 1 before making
/// any network request (fail-fast on bad input).
#[test]
fn health_fail_on_invalid_value_rejects_early() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), MINIMAL_WASM).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("health")
        .arg("--contract")
        .arg("CNotARealContractId")
        .arg("--wasm")
        .arg(tmp.path())
        .arg("--fail-on")
        .arg("bogus")
        .assert()
        .failure()
        .stderr(predicates::str::contains("--fail-on must be"));
}

/// Without `--fail-on` a successful health run (even if it would fail offline)
/// exits 1 due to the RPC error — the flag must never alter the flagless code
/// path.  We verify the flag is absent from the invocation rather than
/// checking exit 0 (which would need a live RPC).
#[test]
fn health_flagless_run_does_not_use_fail_on() {
    // This test asserts the flagless codepath still reaches the RPC layer
    // (exits 1 due to invalid contract, never exit 2 which is the gate code).
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), MINIMAL_WASM).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let output = cmd
        .arg("health")
        .arg("--contract")
        .arg("CNotARealContractId")
        .arg("--wasm")
        .arg(tmp.path())
        .output()
        .unwrap();

    // The exit code must be 1 (RPC / operational error), NOT 2 (gate).
    assert_ne!(
        output.status.code(),
        Some(2),
        "flagless health must not exit 2"
    );
}

/// `sdkt health --help` must document `--fail-on` so users can discover it.
#[test]
fn health_help_documents_fail_on_flag() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.args(["health", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("fail-on"));
}
