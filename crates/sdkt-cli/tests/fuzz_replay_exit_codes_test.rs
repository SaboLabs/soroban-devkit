//! Integration tests for `sdkt fuzz replay` exit codes.
//!
//! Enforces the CLI-wide exit code convention (crates/sdkt-core/src/registry.rs:52-54):
//! - `0`: Success (reproduced finding verdict)
//! - `1`: Runtime/verdict failure (mismatch verdict, unreadable artifact/WASM,
//!        malformed artifact, unsupported schema version, unknown reason code,
//!        or WASM hash mismatch)
//! - `2`: Command-line usage error (unknown flag, missing required argument,
//!        invalid format flag)

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fuzz(args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("built binary");
    cmd.arg("fuzz");
    cmd.args(args);
    cmd
}

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_str()
        .expect("utf-8 fixture path")
        .to_string()
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-fuzz-exit-test-{label}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Produce a valid finding artifact from a campaign against `us_new.wasm`.
fn produce_valid_artifact(dir: &Path) -> PathBuf {
    fuzz(&[
        "campaign",
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--seed",
        "11",
        "--expect-error",
        "hello:Storage:3",
        "--artifact-dir",
        dir.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(1);

    std::fs::read_dir(dir)
        .expect("dir readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("finding-") && n.ends_with(".json"))
        })
        .expect("at least one finding artifact")
}

#[test]
fn replay_nonexistent_artifact_exits_one() {
    fuzz(&[
        "replay",
        "/no/such/nonexistent-artifact.json",
        "--wasm",
        &fixture("us_new.wasm"),
    ])
    .assert()
    .failure()
    .code(1)
    .stderr(predicate::str::contains(
        "reading /no/such/nonexistent-artifact.json",
    ));
}

#[test]
fn replay_malformed_artifact_exits_one() {
    let dir = temp_dir("malformed");
    let path = dir.join("malformed.json");
    std::fs::write(&path, b"not valid json").expect("write malformed");

    fuzz(&[
        "replay",
        path.to_str().unwrap(),
        "--wasm",
        &fixture("us_new.wasm"),
    ])
    .assert()
    .failure()
    .code(1)
    .stderr(predicate::str::contains(
        "artifact JSON could not be parsed",
    ));
}

#[test]
fn replay_unsupported_schema_version_exits_one() {
    let dir = temp_dir("unsupported-schema");
    let valid = produce_valid_artifact(&dir);
    let mut val: Value = serde_json::from_str(&std::fs::read_to_string(&valid).unwrap()).unwrap();
    val["schema_version"] = serde_json::json!(999);
    let tampered_path = dir.join("bad-schema.json");
    std::fs::write(&tampered_path, serde_json::to_string(&val).unwrap()).unwrap();

    fuzz(&[
        "replay",
        tampered_path.to_str().unwrap(),
        "--wasm",
        &fixture("us_new.wasm"),
    ])
    .assert()
    .failure()
    .code(1)
    .stderr(predicate::str::contains(
        "unsupported artifact schema_version 999",
    ));
}

#[test]
fn replay_unknown_reason_code_exits_one() {
    let dir = temp_dir("unknown-reason");
    let valid = produce_valid_artifact(&dir);
    let mut val: Value = serde_json::from_str(&std::fs::read_to_string(&valid).unwrap()).unwrap();
    val["reason_code"] = serde_json::json!("BOGUS");
    let tampered_path = dir.join("bad-reason.json");
    std::fs::write(&tampered_path, serde_json::to_string(&val).unwrap()).unwrap();

    fuzz(&[
        "replay",
        tampered_path.to_str().unwrap(),
        "--wasm",
        &fixture("us_new.wasm"),
    ])
    .assert()
    .failure()
    .code(1)
    .stderr(predicate::str::contains("unknown reason_code `BOGUS`"));
}

#[test]
fn replay_wasm_hash_mismatch_exits_one() {
    let dir = temp_dir("wasm-mismatch");
    let valid = produce_valid_artifact(&dir);

    fuzz(&[
        "replay",
        valid.to_str().unwrap(),
        "--wasm",
        &fixture("us_old.wasm"),
    ])
    .assert()
    .failure()
    .code(1)
    .stderr(predicate::str::contains("wasm hash mismatch"));
}

#[test]
fn replay_missing_wasm_file_exits_one() {
    let dir = temp_dir("missing-wasm");
    let valid = produce_valid_artifact(&dir);

    fuzz(&[
        "replay",
        valid.to_str().unwrap(),
        "--wasm",
        "/no/such/nonexistent.wasm",
    ])
    .assert()
    .failure()
    .code(1)
    .stderr(predicate::str::contains("WASM file could not be read"));
}

#[test]
fn replay_reproduced_verdict_exits_zero() {
    let dir = temp_dir("reproduced");
    let valid = produce_valid_artifact(&dir);

    fuzz(&[
        "replay",
        valid.to_str().unwrap(),
        "--wasm",
        &fixture("us_new.wasm"),
    ])
    .assert()
    .success()
    .code(0)
    .stdout(predicate::str::contains("REPRODUCED"))
    .stdout(predicate::str::contains("RETURN_MISMATCH"));
}

#[test]
fn replay_mismatch_verdict_exits_one() {
    let dir = temp_dir("mismatch");
    let valid = produce_valid_artifact(&dir);
    let mut val: Value = serde_json::from_str(&std::fs::read_to_string(&valid).unwrap()).unwrap();
    // Tamper the expected reason code to a known valid but different reason code
    val["reason_code"] = serde_json::json!("STATE_MISMATCH");
    let tampered_path = dir.join("tampered-reason.json");
    std::fs::write(&tampered_path, serde_json::to_string(&val).unwrap()).unwrap();

    fuzz(&[
        "replay",
        tampered_path.to_str().unwrap(),
        "--wasm",
        &fixture("us_new.wasm"),
    ])
    .assert()
    .failure()
    .code(1)
    .stdout(predicate::str::contains("MISMATCH"));
}

#[test]
fn replay_unknown_flag_is_usage_error_exit_two() {
    fuzz(&["replay", "--unknown-flag"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn replay_invalid_format_is_usage_error_exit_two() {
    let dir = temp_dir("bad-fmt");
    let valid = produce_valid_artifact(&dir);

    fuzz(&[
        "replay",
        valid.to_str().unwrap(),
        "--wasm",
        &fixture("us_new.wasm"),
        "--format",
        "invalid-format",
    ])
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains("invalid format 'invalid-format'"));
}
