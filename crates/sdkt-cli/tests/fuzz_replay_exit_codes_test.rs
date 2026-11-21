use std::env;
use std::path::PathBuf;
use std::process::Command;

/// Helper to build the path to the compiled `sdkt` binary for this test run.
fn sdkt_bin() -> PathBuf {
    // CARGO_BIN_EXE points at the binary under test for integration tests.
    let bin =
        env::var("CARGO_BIN_EXE_sdkt").expect("CARGO_BIN_EXE_sdkt not set; run via cargo test");
    PathBuf::from(bin)
}

/// Run `sdkt fuzz replay` with the given arguments and return the exit code.
fn replay_exit_code(args: &[&str]) -> i32 {
    let output = Command::new(sdkt_bin())
        .args(["fuzz", "replay"])
        .args(args)
        .output()
        .expect("failed to spawn sdkt");
    output.status.code().expect("sdkt terminated by signal")
}

/// Write contents to a temporary file and return its path.
fn write_temp(name: &str, contents: &str) -> PathBuf {
    let dir = env::temp_dir();
    let path = dir.join(format!("sdkt-fuzz-replay-{}-{}", std::process::id(), name));
    std::fs::write(&path, contents).expect("failed to write temp file");
    path
}

/// A minimal valid-shape artifact with an unsupported schema version.
const BAD_SCHEMA: &str = r#"{
    "schema_version": 999,
    "reason_code": "RETURN_MISMATCH",
    "wasm_hash": "0000000000000000000000000000000000000000000000000000000000000000",
    "input": "",
    "expected": "UnexpectedError",
    "actual": "Finding(ReturnMismatch)"
}"#;

/// A minimal artifact with an unknown reason_code.
const BAD_REASON: &str = r#"{
    "schema_version": 1,
    "reason_code": "BOGUS",
    "wasm_hash": "0000000000000000000000000000000000000000000000000000000000000000",
    "input": "",
    "expected": "UnexpectedError",
    "actual": "Finding(ReturnMismatch)"
}"#;

/// A minimal artifact with a WASM hash that will not match any real WASM.
const MISMATCH_HASH: &str = r#"{
    "schema_version": 1,
    "reason_code": "RETURN_MISMATCH",
    "wasm_hash": "0000000000000000000000000000000000000000000000000000000000000000",
    "input": "",
    "expected": "UnexpectedError",
    "actual": "Finding(ReturnMismatch)"
}"#;

/// A trivially valid WASM module (empty module) used to exercise the WASM hash
/// mismatch path without requiring a real fixture.
const EMPTY_WASM: &[u8; 8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

fn write_temp_wasm(name: &str) -> PathBuf {
    let dir = env::temp_dir();
    let path = dir.join(format!(
        "sdkt-fuzz-replay-wasm-{}-{}",
        std::process::id(),
        name
    ));
    std::fs::write(&path, EMPTY_WASM).expect("failed to write temp wasm");
    path
}

#[test]
fn missing_artifact_exits_1() {
    let wasm = write_temp_wasm("missing");
    let code = replay_exit_code(&[
        "--wasm",
        wasm.to_str().unwrap(),
        "/nonexistent-artifact.json",
    ]);
    assert_eq!(code, 1, "missing artifact must exit 1");
}

#[test]
fn malformed_artifact_exits_1() {
    let wasm = write_temp_wasm("malformed");
    let artifact = write_temp("malformed.json", "{ not json");
    let code = replay_exit_code(&[
        "--wasm",
        wasm.to_str().unwrap(),
        artifact.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "malformed artifact must exit 1");
}

#[test]
fn unsupported_schema_exits_1() {
    let wasm = write_temp_wasm("schema");
    let artifact = write_temp("schema.json", BAD_SCHEMA);
    let code = replay_exit_code(&[
        "--wasm",
        wasm.to_str().unwrap(),
        artifact.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "unsupported schema must exit 1");
}

#[test]
fn unknown_reason_code_exits_1() {
    let wasm = write_temp_wasm("reason");
    let artifact = write_temp("reason.json", BAD_REASON);
    let code = replay_exit_code(&[
        "--wasm",
        wasm.to_str().unwrap(),
        artifact.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "unknown reason_code must exit 1");
}

#[test]
fn wasm_hash_mismatch_exits_1() {
    let wasm = write_temp_wasm("hash");
    let artifact = write_temp("hash.json", MISMATCH_HASH);
    let code = replay_exit_code(&[
        "--wasm",
        wasm.to_str().unwrap(),
        artifact.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "WASM hash mismatch must exit 1");
}

#[test]
fn unknown_flag_exits_2() {
    let wasm = write_temp_wasm("usage");
    let code = replay_exit_code(&[
        "--wasm",
        wasm.to_str().unwrap(),
        "--not-a-real-flag",
        "/tmp/irrelevant.json",
    ]);
    assert_eq!(code, 2, "unknown flag must exit 2");
}

#[test]
fn missing_required_argument_exits_2() {
    // No positional artifact and no --wasm: clap should report usage error.
    let code = replay_exit_code(&[]);
    assert_eq!(code, 2, "missing required arguments must exit 2");
}
