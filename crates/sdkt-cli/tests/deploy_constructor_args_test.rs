use assert_cmd::Command;
use predicates::prelude::*;

fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd
}

#[test]
fn cli_deploy_help_displays_arg_flag() {
    let dir = tempfile::tempdir().unwrap();
    sdkt(dir.path())
        .args(["deploy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--arg"))
        .stdout(predicate::str::contains("Constructor argument"));
}

#[test]
fn cli_deploy_rejects_invalid_u32_arg() {
    let dir = tempfile::tempdir().unwrap();
    let wasm_file = dir.path().join("test.wasm");
    std::fs::write(&wasm_file, b"\0asm\x01\0\0\0").unwrap();

    sdkt(dir.path())
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
        .failure()
        .stderr(predicate::str::contains("invalid u32 value"));
}

#[test]
fn cli_deploy_rejects_invalid_bool_arg() {
    let dir = tempfile::tempdir().unwrap();
    let wasm_file = dir.path().join("test.wasm");
    std::fs::write(&wasm_file, b"\0asm\x01\0\0\0").unwrap();

    sdkt(dir.path())
        .args([
            "deploy",
            "--wasm",
            wasm_file.to_str().unwrap(),
            "--identity",
            "default",
            "--arg",
            "bool:neither_true_nor_false",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid bool value"));
}

#[test]
fn cli_deploy_rejects_invalid_bytes_hex() {
    let dir = tempfile::tempdir().unwrap();
    let wasm_file = dir.path().join("test.wasm");
    std::fs::write(&wasm_file, b"\0asm\x01\0\0\0").unwrap();

    sdkt(dir.path())
        .args([
            "deploy",
            "--wasm",
            wasm_file.to_str().unwrap(),
            "--identity",
            "default",
            "--arg",
            "bytes:odd_length_123",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid hex byte"));
}

#[test]
fn cli_deploy_validates_multiple_constructor_args_before_deploy() {
    let dir = tempfile::tempdir().unwrap();
    let wasm_file = dir.path().join("test.wasm");
    std::fs::write(&wasm_file, b"\0asm\x01\0\0\0").unwrap();

    // Valid types should pass arg parsing and proceed to identity/network lookup
    sdkt(dir.path())
        .args([
            "deploy",
            "--wasm",
            wasm_file.to_str().unwrap(),
            "--identity",
            "nonexistent_identity",
            "--arg",
            "u32:42",
            "--arg",
            "string:hello",
            "--arg",
            "symbol:my_symbol",
            "--arg",
            "bool:true",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Identity 'nonexistent_identity' not found",
        ));
}

#[test]
fn cli_deploy_rejects_invalid_passthrough_base64_arg_before_identity() {
    let dir = tempfile::tempdir().unwrap();
    let wasm_file = dir.path().join("test.wasm");
    std::fs::write(&wasm_file, b"\0asm\x01\0\0\0").unwrap();

    // Invalid base64 or invalid ScVal should fail immediately before looking up identity
    sdkt(dir.path())
        .args([
            "deploy",
            "--wasm",
            wasm_file.to_str().unwrap(),
            "--identity",
            "nonexistent_identity",
            "--arg",
            "not_valid_scval_base64!?",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid constructor argument"));
}
