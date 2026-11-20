use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn sdkt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sdkt"))
}

fn temp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let unique = format!(
        "sdkt-fuzz-replay-test-{}-{}-{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    p.push(unique);
    p
}

fn write_temp(name: &str, contents: &[u8]) -> PathBuf {
    let p = temp_path(name);
    fs::write(&p, contents).expect("write temp file");
    p
}

fn minimal_wasm() -> Vec<u8> {
    // Empty valid WASM module.
    vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
}

fn run_replay(wasm: &PathBuf, artifact: &PathBuf) -> i32 {
    let out = sdkt()
        .arg("the fuzz")
        .arg("replay")
        .arg("--wasm")
        .arg(wasm)
        .arg(artifact)
        .output()
        .expect("run sdkt");
    out.status.code().unwrap_or(-1)
}

#[test]
fn replay_missing_artifact_exits_1() {
    let wasm = write_temp("wasm", &minimal_wasm());
    let missing = temp_path("missing-artifact");
    assert_eq(
        run_replay(&wasm, &missing),
        1,
        "missing artifact should exit 1"
    );
    let _ = fs::remove_file(&wasm);
}

#[test]
fn replay_malformed_artifact_exits_1() {
    let wasm = write_temp("wasm", &minimal_wasm());
    let bad = write_temp("bad-artifact", b{"{not json"});
    assert_eq(
        run_replay(&wasm, &bad),
        1,
        "malformed artifact should exit 1"
    );
    let _ = fs::remove_file(&wasm);
    let _ = fs::remove_file(&bad);
}

#[test]
fn replay_unsupported_schema_exits_1() {
    let wasm = write_temp("wasm", &minimal_wasm());
    let bad = write_temp(
        "bad-schema",
        b{"{\"schema_version\":999}"},
    );
    assert_eq(
        run_replay(&wasm, &bad),
        1,
        "unsupported schema should exit 1"
    );
    let _ = fs::remove_file(&wasm);
    let _ = fs::remove_file(&bad);
}

#[test]
fn replay_unknown_reason_code_exits_1() {
    let wasm = write_temp("wasm", &minimal_wasm());
    let bad = write_temp(
        "bad-reason",
        b{'{"schema_version":1,"reason_code":"BOGUS"}'.to_string().as_bytes(),
    );
    assert_eq(
        run_replay(&wasm, &bad),
        1,
        "unknown reason_code should exit 1"
    );
    let _ = fs::remove_file(&wasm);
    let _ = fs::remove_file(&bad);
}

#[test]
fn replay_wasm_hash_mismatch_exits_1() {
    let wasm = write_temp("wasm", &minimal_wasm());
    // Artifact claiming a hash that cannot match the supplied WASM.
    let bad = write_temp(
        "hash-mismatch",
        b{"{\"schema_version\":1,\"wasm_hash\":\"00000000000000000000000000000000000000000000000000000000000000000\"}"},
    );
    assert_eq(
        run_replay(&wasm, &bad),
        1,
        "wasm hash mismatch should exit 1"
    );
    let _ = fs::remove_file(&wasm);
    let _ = fs::remove_file(&bad);
}

#[test]
fn replay_unknown_flag_exits_2() {
    let out = sdkt()
        .arg("fuzz")
        .arg("replay")
        .arg("--this-flag-does-not-exist")
        .output()
        .expect("run sdkt");
    assert_eq(
        out.status.code().unwrap_or(-1),
        2,
        "unknown flag should exit 2"
    );
}
