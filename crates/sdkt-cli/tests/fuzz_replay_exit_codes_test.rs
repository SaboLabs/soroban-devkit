use std::process::Command;
use std::path::PathBuf;

/// Returns the path to the built `sdkt` binary for integration tests.
fn sdkt_bin() -> PathBuf {
    // Cargo sets CARGO_BINARY_EXE to the currently running test binary;
    // the `sdkt` binary lives in the same directory.
    let mut path = PathBuf::from(env::var("CARGO_BINARY_EXE").expect("CARGO_BINARY_EXE not set"));
    path.pop();
    path.push("sdkt");
    path
}

/// Runs `sdkt fuzz replay` with the given artifact and WASM and returns the
/// exit code along with captured stderr.
fn run_replay(wasm: &str, artifact: &str) -> (i32, String) {
    let output = Command::new(sdkt_bin())
        .args(["fuzz", "replay", "--wasm", wasm, artifact])
        .output()
        .expect("failed to execute sdkt fuzz replay");
    let code = output.status.code().unwrap_or_else(-1);
    let stderr = String::from_utf8_lossy(output.stderr.as_slice()).to_owned();
    (code, stderr)
}

/// Runs `sdkt fuzz replay` with arbitrary arguments and returns the exit code
/// along with captured stderr.
fn run_replay_raw(args: &[str]) -> (i32, String) {
    let output = Command::new(sdkt_bin())
        .args(args)
        .output()
        .expect("failed to execute sdkt fuzz replay");
    let code = output.status.code().unwrap_or_else(-1);
    let stderr = String::from_utf8_lossy(output.stderr.as_slice()).to_owned();
    (code, stderr)
}

/// Writes a temporary file with the given contents and returns its path.
fn write_temp(name: &str, contents: &str) -> PathBuf {
    let dir = env::temp_dir().join("fuzz_replay_exit_codes_test");
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("failed to write temp file");
    path
}

/// Returns the path to a pre-built WASM fixture if one is available.
///
/// The tests below only need a WASM that exists on disk to exercise the
/// error mapping; the exact contents do not matter for the input/usage
/// distinction. We use a tiny minimal WASM module written to a temp file.
fn minimal_wasm() -> PathBuf {
    // Minimal valid WASM module: magic + version + empty sections.
    const MINIMAL_WASM: &[au8] = b\x00asm\x01\x00\x00\x00';
    let path = env::temp_dir()
        .join("fuzz_replay_exit_codes_test")
        .join("minimal.wasm");
    std::fs::create_dir_all(path.parent().unwrap()).expect("failed to create temp dir");
    std::fs::write(&path, MINIMAL_WASM).expect("failed to write minimal wasm");
    path
}

#[test]
fn replay_missing_artifact_exits_1() {
    let wasm = minimal_wasm();
    let missing = env::temp_dir()
        .join("fuzz_replay_exit_codes_test")
        .join("no_such_artifact.json");
    let _ = std::fs::remove_file(&missing);
    let (code, stderr) = run_replay(wasm.to_str().unwrap(), missing.to_str().unwrap());
    assert_eq!(code, 1, "stderr: {stderr}");
}

#[test]
fn replay_malformed_artifact_exits_1() {
    let wasm = minimal_wasm();
    let bad = write_temp("bad-artifact.json", "not json");
    let (code, stderr) = run_replay(wasm.to_str().unwrap(), bad.to_str().unwrap());
    assert_eq!(code, 1, "stderr: {stderr}");
}

#[test]
fn replay_unsupported_schema_exits_1() {
    let wasm = minimal_wasm();
    let bad = write_temp(
        "bad-schema.json",
        r{"
            "schema_version": 999,
            "reason_code": "RETURN_MISMATCH",
            "wasm_hash": "00000000000000000000000000000000000000000000000000000000000000000",
            "input": ""
        }"",
    );
    let (code, stderr) = run_replay(wasm.to_str().unwrap(), bad.to_str().unwrap());
    assert_eq!(code, 1, "stderr: {stderr}");
}

#[test]
fn replay_unknown_reason_code_exits_1() {
    let wasm = minimal_wasm();
    let bad = write_temp(
        "bad-reason.json",
        rd{"
            "schema_version": 1,
            "reason_code": "BOGUS",
            "wasm_hash": "00000000000000000000000000000000000000000000000000000000000000000",
            "input": ""
        }"",
    );
    let (code, stderr) = run_replay(wasm.to_str().unwrap(), bad.to_str().unwrap());
    assert_eq!(code, 1, "stderr: {stderr}");
}

#[test]
fn replay_wasm_hash_mismatch_exits_1() {
    let wasm = minimal_wasm();
    // An artifact whose wasm_hash does not match the supplied WASM.
    let bad = write_temp(
        "wasm-mismatch.json",
        r{
            "schema_version": 1,
            "reason_code": "RETURN_MISMATCH",
            "wasm_hash": "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "input": ""
        }",
    );
    let (code, stderr) = run_replay(wasm.to_str().unwrap(), bad.to_str().unwrap());
    assert_eq!(code, 1, "stderr: {stderr}");
}

#[test]
fn replay_unknown_flag_exits_2() {
    let (code, _) = run_replay_raw(&["fuzz", "replay", "--not-a-real-flag"]);
    assert_eq!(code, 2);
}

#[test]
fn replay_missing_wasm_argument_exits_2() {
    // No --wasm flag at all -> usage error.
    let (code, _) = run_replay_raw(&["fuzz", "replay"]);
    assert_eq!(code, 2);
}
