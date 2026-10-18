//! CLI regression tests for `[decode]` config wiring (issue #375, reviewer
//! round-trip). Offline and deterministic: writes a `.sdkt.toml` into a temp
//! directory, points `sdkt decode` at it via the working directory, and asserts
//! that `max_depth` and `allow_fallback_hex` are honoured on the CLI path.
//!
//! The payload is a deliberately nested ScVal: Vec[Vec[Vec[Vec[U32(7)]]]]
//! = XDR depth 4.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

/// Base64 of Vec[Vec[Vec[Vec[U32(7)]]]] — depth 4.
const DEEP_SCVAL_B64: &str =
    "AAAAEAAAAAEAAAABAAAAEAAAAAEAAAABAAAAEAAAAAEAAAABAAAAEAAAAAEAAAABAAAAAwAAAAc=";

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env_remove("HOME");
    cmd
}

fn write_config(dir: &TempDir, toml: &str) {
    fs::write(dir.path().join(".sdkt.toml"), toml).expect("write .sdkt.toml");
}

/// `max_depth = 0` means unlimited, so a depth-4 payload must decode
/// successfully — proving the documented 0=unlimited semantics are honoured.
#[test]
fn cli_decode_max_depth_zero_is_unlimited() {
    let dir = TempDir::new().unwrap();
    write_config(&dir, "[decode]\nmax_depth = 0\nallow_fallback_hex = true\n");

    let mut cmd = sdkt();
    cmd.current_dir(dir.path()).args([
        "decode",
        DEEP_SCVAL_B64,
        "--type",
        "ScVal",
        "--format",
        "json",
    ]);
    let out = cmd.output().expect("decode runs");
    assert!(
        out.status.success(),
        "max_depth=0 must be unlimited, got stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let decoded = String::from_utf8(out.stdout).unwrap();
    assert!(
        decoded.contains("7"),
        "decoded ScVal must carry the u32 value 7, got: {decoded}"
    );
}

/// `max_depth = 2` must reject the depth-4 payload on the CLI path.
#[test]
fn cli_decode_max_depth_rejects_too_deep_payload() {
    let dir = TempDir::new().unwrap();
    write_config(&dir, "[decode]\nmax_depth = 2\nallow_fallback_hex = true\n");

    let mut cmd = sdkt();
    cmd.current_dir(dir.path())
        .args([
            "decode",
            DEEP_SCVAL_B64,
            "--type",
            "ScVal",
            "--format",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("depth limit exceeded"));
}

/// An omitted `[decode]` section uses the default (500) → depth-4 decodes fine.
#[test]
fn cli_decode_default_config_accepts_shallow_payload() {
    let dir = TempDir::new().unwrap();
    write_config(&dir, "[storage]\nmax_entries = 10\n");

    let mut cmd = sdkt();
    cmd.current_dir(dir.path())
        .args([
            "decode",
            DEEP_SCVAL_B64,
            "--type",
            "ScVal",
            "--format",
            "json",
        ])
        .assert()
        .success();
}

/// `allow_fallback_hex = false` must reject a bare-hex (non-base64) payload.
#[test]
fn cli_decode_allow_fallback_hex_false_rejects_hex_input() {
    let dir = TempDir::new().unwrap();
    write_config(
        &dir,
        "[decode]\nmax_depth = 500\nallow_fallback_hex = false\n",
    );

    let mut cmd = sdkt();
    cmd.current_dir(dir.path())
        .args(["decode", "deadbeef", "--type", "ScVal", "--format", "json"])
        .assert()
        .failure();
}
