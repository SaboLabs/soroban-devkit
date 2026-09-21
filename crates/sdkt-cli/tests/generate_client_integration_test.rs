use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

const TEST_WASM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_old.wasm");

#[test]
fn generate_client_writes_file() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("client.rs");
    sdkt()
        .args(["generate", "client", TEST_WASM, "-o"])
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("Generated client"));
    let code = fs::read_to_string(&out).unwrap();
    assert!(code.contains("pub struct TransferCall"));
    assert!(code.contains("pub struct MintCall"));
}

#[test]
fn generate_client_prints_to_stdout() {
    let assert = sdkt().args(["generate", "client", TEST_WASM]).assert();
    assert
        .success()
        .stdout(predicate::str::contains("pub struct TransferCall"))
        .stdout(predicate::str::contains("pub fn contract_functions()"));
}

#[test]
fn generate_client_rejects_missing_file() {
    sdkt()
        .args(["generate", "client", "/nonexistent.wasm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read WASM"));
}

#[test]
fn generate_client_rejects_non_contract_wasm() {
    let tmp = TempDir::new().unwrap();
    let plain = tmp.path().join("plain.wasm");
    fs::write(&plain, b"\x00asm\x01\x00\x00\x00").unwrap();
    sdkt()
        .args(["generate", "client"])
        .arg(&plain)
        .assert()
        .failure()
        .stderr(predicate::str::contains("No contractspecv0 section found"));
}

#[test]
fn generate_client_output_is_deterministic() {
    let tmp = TempDir::new().unwrap();
    let out1 = tmp.path().join("a.rs");
    let out2 = tmp.path().join("b.rs");
    sdkt()
        .args(["generate", "client", TEST_WASM, "-o"])
        .arg(&out1)
        .assert()
        .success();
    sdkt()
        .args(["generate", "client", TEST_WASM, "-o"])
        .arg(&out2)
        .assert()
        .success();
    let a = fs::read_to_string(&out1).unwrap();
    let b = fs::read_to_string(&out2).unwrap();
    assert_eq!(a, b, "generated output must be deterministic");
}

#[test]
fn generate_client_help() {
    sdkt()
        .args(["generate", "client", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("typed Rust client"))
        .stdout(predicate::str::contains("--output"));
}

#[test]
fn generate_help_lists_client() {
    sdkt()
        .args(["generate", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("client"));
}
