//! Integration tests for `sdkt tx sign` ( / PR2).
//!
//! These exercise the full CLI binary against an isolated identity keystore
//! (via a temporary `SDKT_IDENTITY_DIR`) so they never touch the developer's
//! real identities. Works cross-platform (Linux, macOS, Windows).

use std::io::Write;
use std::process::Command;

/// Build an unsigned transaction envelope on disk using the CLI, into `dir`.
fn build_unsigned(dir: &std::path::Path) -> std::path::PathBuf {
    let unsigned = dir.join("unsigned.xdr");
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_IDENTITY_DIR", dir)
        .args([
            "tx",
            "build",
            "--source",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "--sequence",
            "12345",
            "--contract",
            "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
            "--function",
            "hello",
            "--output",
        ])
        .arg(&unsigned)
        .output()
        .expect("failed to run sdkt tx build");
    assert!(
        out.status.success(),
        "tx build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    unsigned
}

fn gen_identity(dir: &std::path::Path, name: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_IDENTITY_DIR", dir)
        .args(["identity", "generate", name])
        .output()
        .expect("failed to run sdkt identity generate");
    assert!(
        out.status.success(),
        "identity generate failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_sign(dir: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_IDENTITY_DIR", dir)
        .args(["tx", "sign"])
        .args(args)
        .output()
        .expect("failed to run sdkt tx sign");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn successful_sign_to_file() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    gen_identity(&dir, "alice");
    let unsigned = build_unsigned(&dir);
    let signed = dir.join("signed.xdr");

    let (ok, stdout, stderr) = run_sign(
        &dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--output",
            signed.to_str().unwrap(),
            "--identity",
            "alice",
            "--network",
            "testnet",
        ],
    );
    assert!(ok, "sign should succeed, stderr: {}", stderr);
    assert!(stdout.contains("written to"), "stdout: {}", stdout);

    let content = std::fs::read_to_string(&signed).unwrap();
    // A signed envelope is longer than the unsigned one (a 64-byte sig + hint).
    assert!(content.trim().len() > 50, "signed envelope too short");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn successful_sign_to_stdout() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_stdout_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    gen_identity(&dir, "alice");
    let unsigned = build_unsigned(&dir);

    let (ok, stdout, stderr) = run_sign(
        &dir,
        &["--input", unsigned.to_str().unwrap(), "--identity", "alice"],
    );
    assert!(ok, "sign should succeed, stderr: {}", stderr);
    assert!(
        stdout.contains("Signed Transaction Envelope"),
        "stdout: {}",
        stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn successful_sign_json_format() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_json_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    gen_identity(&dir, "alice");
    let unsigned = build_unsigned(&dir);

    let (ok, stdout, stderr) = run_sign(
        &dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--identity",
            "alice",
            "--format",
            "json",
        ],
    );
    assert!(ok, "sign should succeed, stderr: {}", stderr);
    assert!(
        stdout.trim_start().starts_with("{\"envelope\":"),
        "json stdout: {}",
        stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_identity_errors() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_unknown_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let unsigned = build_unsigned(&dir);

    let (ok, _, stderr) = run_sign(
        &dir,
        &["--input", unsigned.to_str().unwrap(), "--identity", "ghost"],
    );
    assert!(!ok, "sign with unknown identity must fail");
    assert!(
        stderr.contains("unknown identity 'ghost'"),
        "stderr: {}",
        stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_identity_errors() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_missing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let unsigned = build_unsigned(&dir);

    let (ok, _, stderr) = run_sign(
        &dir,
        &["--input", unsigned.to_str().unwrap(), "--identity", ""],
    );
    assert!(!ok, "sign with empty identity must fail");
    assert!(stderr.contains("missing identity"), "stderr: {}", stderr);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_file_errors() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_file_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    gen_identity(&dir, "alice");

    let (ok, _, stderr) = run_sign(
        &dir,
        &["--input", "/no/such/file.xdr", "--identity", "alice"],
    );
    assert!(!ok, "sign with missing file must fail");
    assert!(stderr.contains("invalid file"), "stderr: {}", stderr);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_network_errors() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_net_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    gen_identity(&dir, "alice");
    let unsigned = build_unsigned(&dir);

    let (ok, _, stderr) = run_sign(
        &dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--identity",
            "alice",
            "--network",
            "bogusnet",
        ],
    );
    assert!(!ok, "sign with invalid network must fail");
    assert!(stderr.contains("invalid network"), "stderr: {}", stderr);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_base64_errors() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_b64_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    gen_identity(&dir, "alice");

    let (ok, _, stderr) = run_sign(
        &dir,
        &["--input", "!!!not base64!!!", "--identity", "alice"],
    );
    assert!(!ok, "sign with invalid base64 must fail");
    assert!(stderr.contains("invalid base64"), "stderr: {}", stderr);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_envelope_errors() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_env_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    gen_identity(&dir, "alice");

    // Valid base64 that is NOT a transaction envelope ("hello world" -> aGVsbG8gd29ybGQ=).
    let tmp = dir.join("notenv.txt");
    let mut f = std::fs::File::create(&tmp).unwrap();
    write!(f, "aGVsbG8gd29ybGQ=").unwrap();
    drop(f);

    let (ok, _, stderr) = run_sign(
        &dir,
        &["--input", tmp.to_str().unwrap(), "--identity", "alice"],
    );
    assert!(!ok, "sign with invalid envelope must fail");
    assert!(stderr.contains("invalid envelope"), "stderr: {}", stderr);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cannot_write_output_errors() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_write_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    gen_identity(&dir, "alice");
    let unsigned = build_unsigned(&dir);

    let (ok, _, stderr) = run_sign(
        &dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--identity",
            "alice",
            "--output",
            "/no/such/dir/out.xdr",
        ],
    );
    assert!(!ok, "sign to unwritable output must fail");
    assert!(stderr.contains("cannot write output"), "stderr: {}", stderr);
    let _ = std::fs::remove_dir_all(&dir);
}

fn run_sign_with_network_dir(
    dir: &std::path::Path,
    network_dir: &std::path::Path,
    args: &[&str],
) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_IDENTITY_DIR", dir)
        .env("SDKT_NETWORK_DIR", network_dir)
        .args(["tx", "sign"])
        .args(args)
        .output()
        .expect("failed to run sdkt tx sign");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn add_network_profile(network_dir: &std::path::Path, name: &str, passphrase: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_NETWORK_DIR", network_dir)
        .args([
            "network",
            "add",
            name,
            "--rpc-url",
            "https://rpc.example.com",
            "--passphrase",
            passphrase,
        ])
        .output()
        .expect("failed to run sdkt network add");
    assert!(
        out.status.success(),
        "network add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn sign_with_network_profile_testnet_passphrase() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_prof_tn_{}", std::process::id()));
    let net_dir = dir.join("networks");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&net_dir).unwrap();
    gen_identity(&dir, "alice");
    add_network_profile(&net_dir, "mynet", "Test SDF Network ; September 2015");
    let unsigned = build_unsigned(&dir);
    let signed = dir.join("signed.xdr");

    let (ok, stdout, stderr) = run_sign_with_network_dir(
        &dir,
        &net_dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--output",
            signed.to_str().unwrap(),
            "--identity",
            "alice",
            "--network-profile",
            "mynet",
        ],
    );
    assert!(ok, "sign with network-profile should succeed, stderr: {}", stderr);
    assert!(stdout.contains("written to"), "stdout: {}", stdout);
    assert!(
        std::fs::read_to_string(&signed).unwrap().trim().len() > 50,
        "signed envelope too short"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sign_with_network_profile_custom_passphrase() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_prof_cu_{}", std::process::id()));
    let net_dir = dir.join("networks");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&net_dir).unwrap();
    gen_identity(&dir, "alice");
    add_network_profile(&net_dir, "sandbox", "My Custom Passphrase ; 2026");
    let unsigned = build_unsigned(&dir);

    let (ok, stdout, stderr) = run_sign_with_network_dir(
        &dir,
        &net_dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--identity",
            "alice",
            "--network-profile",
            "sandbox",
        ],
    );
    assert!(ok, "sign with custom profile should succeed, stderr: {}", stderr);
    assert!(
        stdout.contains("Signed Transaction Envelope"),
        "stdout: {}",
        stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sign_network_and_profile_mutually_exclusive() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_excl_{}", std::process::id()));
    let net_dir = dir.join("networks");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&net_dir).unwrap();
    gen_identity(&dir, "alice");
    add_network_profile(&net_dir, "mynet", "Test SDF Network ; September 2015");
    let unsigned = build_unsigned(&dir);

    let (ok, _, stderr) = run_sign_with_network_dir(
        &dir,
        &net_dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--identity",
            "alice",
            "--network",
            "testnet",
            "--network-profile",
            "mynet",
        ],
    );
    assert!(!ok, "both --network and --network-profile must fail");
    assert!(
        stderr.contains("mutually exclusive"),
        "stderr: {}",
        stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sign_unknown_network_profile_errors() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_ghost_{}", std::process::id()));
    let net_dir = dir.join("networks");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&net_dir).unwrap();
    gen_identity(&dir, "alice");
    let unsigned = build_unsigned(&dir);

    let (ok, _, stderr) = run_sign_with_network_dir(
        &dir,
        &net_dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--identity",
            "alice",
            "--network-profile",
            "ghost",
        ],
    );
    assert!(!ok, "unknown profile must fail");
    assert!(
        stderr.contains("not found"),
        "stderr: {}",
        stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sign_network_profile_matches_network_label() {
    // A profile whose passphrase is the Testnet passphrase must produce the
    // same signed envelope as `--network testnet`.
    let dir = std::env::temp_dir().join(format!("sdkt_sign_equiv_{}", std::process::id()));
    let net_dir = dir.join("networks");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&net_dir).unwrap();
    gen_identity(&dir, "alice");
    add_network_profile(&net_dir, "tn", "Test SDF Network ; September 2015");
    let unsigned = build_unsigned(&dir);
    let via_label = dir.join("via_label.xdr");
    let via_profile = dir.join("via_profile.xdr");

    let (ok1, _, stderr1) = run_sign_with_network_dir(
        &dir,
        &net_dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--output",
            via_label.to_str().unwrap(),
            "--identity",
            "alice",
            "--network",
            "testnet",
        ],
    );
    assert!(ok1, "label sign failed: {}", stderr1);

    let (ok2, _, stderr2) = run_sign_with_network_dir(
        &dir,
        &net_dir,
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--output",
            via_profile.to_str().unwrap(),
            "--identity",
            "alice",
            "--network-profile",
            "tn",
        ],
    );
    assert!(ok2, "profile sign failed: {}", stderr2);

    let a = std::fs::read_to_string(&via_label).unwrap();
    let b = std::fs::read_to_string(&via_profile).unwrap();
    assert_eq!(a, b, "profile-resolved Testnet must match --network testnet");
    let _ = std::fs::remove_dir_all(&dir);
}
