//! Offline tests for the Testnet smoke path's CLI prerequisites.
//!
//! These validate the *local, network-free* pieces the manual
//! `testnet-smoke.yml` depends on:
//!   - `identity import <name> -` reads the secret from stdin (keeps the
//!     secret out of argv / `ps` output, which is the CI-safe import path),
//!   - a malformed secret is rejected without echoing it back,
//!   - the smoke workflow/script/fixture are committed, manual-only, and carry
//!     no key material.
//!
//! They deliberately do NOT assert anything about a real Testnet deployment;
//! that is the job of the manual workflow, not a unit test.

use assert_cmd::Command;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};

/// The sdkt binary under test, plus isolated identity/network stores.
fn sdkt_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

/// A syntax-valid, throwaway secret strkey produced *at test time* by the CLI
/// itself. Generating it (rather than committing a literal) means the
/// repository never carries key material, and the value is never real: no
/// account exists for it and nothing funds it.
///
/// The returned `TempDir` owns the generated keystore and must be kept alive
/// for as long as the secret is used.
fn throwaway_secret() -> (tempfile::TempDir, String) {
    let holder = tempfile::tempdir().unwrap();
    let identity_dir = holder.path().join("identity");
    sdkt_cmd(&holder.path().join("root"))
        .env("SDKT_IDENTITY_DIR", &identity_dir)
        .args(["identity", "generate", "seed"])
        .assert()
        .success();

    let toml = std::fs::read_to_string(identity_dir.join("seed.toml")).expect("seed key file");
    let secret = secret_from_toml(&toml).expect("secret_key line");
    (holder, secret)
}

fn secret_from_toml(toml: &str) -> Option<String> {
    toml.lines().find_map(|l| {
        l.trim()
            .strip_prefix("secret_key = \"")
            .and_then(|r| r.strip_suffix('"'))
            .map(str::to_string)
    })
}

/// A syntactically invalid strkey, used to prove the import path rejects it
/// without echoing the input back.
const MALFORMED_SECRET: &str = "NOT_A_VALID_SECRET_AT_ALL";

#[test]
fn identity_import_reads_secret_from_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let (_seed_holder, secret) = throwaway_secret();

    // Raw std::process so stdin can actually be piped (assert_cmd's Command
    // does not expose a stdin builder).
    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("sdkt"))
        .args(["identity", "import", "smoke", "-"])
        .env("SDKT_IDENTITY_DIR", dir.path().join("identity"))
        .env("SDKT_NETWORK_DIR", dir.path().join("network"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sdkt");

    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(secret.as_bytes())
        .expect("write secret");
    let out = child.wait_with_output().expect("wait");

    assert!(out.status.success(), "import should succeed via stdin");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Public Key: G"),
        "expected a G-address, got: {stdout}"
    );
    assert!(!stdout.contains(&secret), "stdout leaked the secret");

    // The imported key must match the generated one.
    let imported =
        std::fs::read_to_string(dir.path().join("identity/smoke.toml")).expect("imported key file");
    assert_eq!(
        secret_from_toml(&imported).as_deref(),
        Some(secret.as_str()),
        "imported key differs from the piped secret"
    );
}

#[test]
fn identity_import_stdin_rejects_malformed_secret_without_echo() {
    let dir = tempfile::tempdir().unwrap();

    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("sdkt"))
        .args(["identity", "import", "smoke", "-"])
        .env("SDKT_IDENTITY_DIR", dir.path().join("identity"))
        .env("SDKT_NETWORK_DIR", dir.path().join("network"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sdkt");

    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(MALFORMED_SECRET.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");

    assert!(!out.status.success(), "malformed secret must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !combined.contains(MALFORMED_SECRET),
        "error output echoed the input secret: {combined}"
    );
}

#[test]
fn identity_import_argv_still_works() {
    // Backwards compatibility: the positional secret path is unchanged.
    let dir = tempfile::tempdir().unwrap();
    let (_seed_holder, secret) = throwaway_secret();
    sdkt_cmd(dir.path())
        .args(["identity", "import", "legacy", &secret])
        .assert()
        .success();
}

#[test]
fn smoke_path_files_are_committed_and_manual_only() {
    let repo = repo_root();

    let wf = repo.join(".github/workflows/testnet-smoke.yml");
    assert!(wf.exists(), "missing {wf:?}");
    let wf_text = std::fs::read_to_string(&wf).unwrap();
    assert!(
        wf_text.contains("workflow_dispatch"),
        "workflow must be manual-dispatch"
    );
    assert!(
        !has_ci_trigger(&wf_text),
        "workflow must not run on pull_request or push"
    );
    assert!(
        wf_text.contains("secrets.STELLAR_TESTNET_SECRET"),
        "workflow must source the secret from GitHub Secrets"
    );

    let script = repo.join("tests/smoke/testnet-deploy-smoke.sh");
    assert!(script.exists(), "missing {script:?}");
    let script_text = std::fs::read_to_string(&script).unwrap();
    for phrase in [
        "TESTNET NOT TESTED",
        "TESTNET GUARD FAILED",
        "INFRASTRUCTURE FAILURE",
        "DEPLOYMENT FAILURE",
        "VERIFICATION FAILURE",
        "SUCCESS",
        "Test SDF Network ; September 2015",
        // The Testnet guard must ask the node, not just trust the profile.
        "getNetwork",
    ] {
        assert!(script_text.contains(phrase), "script missing {phrase:?}");
    }
    // The secret must only ever reach the CLI on stdin, never on argv.
    assert!(
        script_text.contains("identity import smoke -"),
        "script must import the secret via stdin (dash placeholder)"
    );
    assert!(
        !script_text.contains("identity import smoke \"$SECRET\""),
        "script must not pass the secret on argv"
    );

    let fixture_manifest = repo.join("tests/fixtures/testnet-smoke/Cargo.toml");
    assert!(fixture_manifest.exists(), "missing fixture manifest");
    let fx = std::fs::read_to_string(&fixture_manifest).unwrap();
    assert!(
        fx.contains("publish = false"),
        "fixture must not be publishable"
    );
    assert!(fx.contains("soroban-sdk"), "fixture pins soroban-sdk");

    // Nothing committed may look like a real secret strkey.
    assert!(!has_secret_strkey(&script_text), "script embeds a key");
    assert!(!has_secret_strkey(&wf_text), "workflow embeds a key");
}

fn repo_root() -> PathBuf {
    // crates/sdkt-cli -> repository root
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// True if the workflow declares a `pull_request:` or `push:` trigger.
///
/// Only top-level trigger keys count; prose in comments that merely names
/// those events is fine.
fn has_ci_trigger(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim_start();
        !t.starts_with('#') && (t.starts_with("pull_request:") || t.starts_with("push:"))
    })
}

/// True if the text contains a Stellar secret strkey (`S` + 55 base32 chars).
fn has_secret_strkey(text: &str) -> bool {
    const B32: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let chars: Vec<char> = text.chars().collect();
    'outer: for i in 0..chars.len().saturating_sub(55) {
        if chars[i] != 'S' {
            continue;
        }
        for k in &chars[i + 1..i + 56] {
            if !B32.contains(*k) {
                continue 'outer;
            }
        }
        return true;
    }
    false
}
