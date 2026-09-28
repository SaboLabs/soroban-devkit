//! A non-ASCII `bytes:` argument must fail with the same clean error that
//! `sdkt encode` gives, on every command that parses typed arguments. The
//! value is rejected before any network call, so no RPC server is needed.

use assert_cmd::Command;
use predicates::prelude::*;

const CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const SOURCE: &str = "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN7";
const BAD_BYTES: &str = "bytes:a€";
const EXPECTED: &str = "invalid bytes value: a€ (expected ASCII hex)";
// Nothing listens here; reaching the network would fail with a different error.
const DEAD_RPC: &str = "http://127.0.0.1:9";

fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd
}

fn assert_clean_rejection(dir: &std::path::Path, args: &[&str]) {
    sdkt(dir)
        .args(args)
        .assert()
        .code(1)
        .stderr(predicate::str::contains(EXPECTED))
        .stderr(predicate::str::contains("panicked").not());
}

#[test]
fn encode_rejects_non_ascii_bytes() {
    let dir = tempfile::tempdir().unwrap();
    assert_clean_rejection(dir.path(), &["encode", BAD_BYTES]);
}

#[test]
fn tx_build_rejects_non_ascii_bytes() {
    let dir = tempfile::tempdir().unwrap();
    assert_clean_rejection(
        dir.path(),
        &[
            "tx",
            "build",
            "--source",
            SOURCE,
            "--contract",
            CONTRACT,
            "--function",
            "f",
            "--arg",
            BAD_BYTES,
        ],
    );
}

#[test]
fn call_rejects_non_ascii_bytes() {
    let dir = tempfile::tempdir().unwrap();
    assert_clean_rejection(
        dir.path(),
        &[
            "call",
            CONTRACT,
            "f",
            "--rpc-url",
            DEAD_RPC,
            "--args",
            BAD_BYTES,
        ],
    );
}

#[test]
fn invoke_rejects_non_ascii_bytes() {
    let dir = tempfile::tempdir().unwrap();
    sdkt(dir.path())
        .args(["identity", "generate", "alice"])
        .assert()
        .success();
    assert_clean_rejection(
        dir.path(),
        &[
            "invoke",
            CONTRACT,
            "f",
            "--identity",
            "alice",
            "--rpc-url",
            DEAD_RPC,
            "--args",
            BAD_BYTES,
        ],
    );
}

#[test]
fn deploy_rejects_non_ascii_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let wasm = dir.path().join("x.wasm");
    std::fs::write(&wasm, b"\0asm\x01\0\0\0").unwrap();
    assert_clean_rejection(
        dir.path(),
        &[
            "deploy",
            "--wasm",
            wasm.to_str().unwrap(),
            "--rpc-url",
            DEAD_RPC,
            "--arg",
            BAD_BYTES,
        ],
    );
}

#[test]
fn storage_read_rejects_non_ascii_key_arg() {
    let dir = tempfile::tempdir().unwrap();
    assert_clean_rejection(
        dir.path(),
        &[
            "storage",
            "read",
            "--contract",
            CONTRACT,
            "--map-key",
            "k",
            "--key-arg",
            BAD_BYTES,
        ],
    );
}
