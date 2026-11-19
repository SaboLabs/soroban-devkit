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

/// Register `name` as the keystore default so `tx sign` can omit `--identity`.
fn set_default_identity(dir: &std::path::Path, name: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_IDENTITY_DIR", dir)
        .args(["identity", "default", name])
        .output()
        .expect("failed to run sdkt identity default");
    assert!(
        out.status.success(),
        "identity default failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_sign(dir: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_IDENTITY_DIR", dir)
        .env("SDKT_NETWORK_DIR", dir)
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

fn add_network_profile(dir: &std::path::Path, name: &str, rpc_url: &str, passphrase: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_NETWORK_DIR", dir)
        .args([
            "network",
            "add",
            name,
            "--rpc-url",
            rpc_url,
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

fn parse_envelope_b64(b64: &str) -> stellar_xdr::TransactionEnvelope {
    use base64::Engine;
    use stellar_xdr::{Limited, Limits, ReadXdr};

    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .expect("signed envelope must be valid base64");
    let mut cursor = std::io::Cursor::new(&raw);
    let mut limited = Limited::new(&mut cursor, Limits::none());
    stellar_xdr::TransactionEnvelope::read_xdr(&mut limited)
        .expect("signed envelope must be valid XDR")
}

fn load_signer(dir: &std::path::Path, name: &str) -> sdkt_xdr::Ed25519Signer {
    let store = sdkt_storage::IdentityStore::with_dir(dir).expect("open identity store");
    let key = store.load_signing_key(name).expect("load signing key");
    sdkt_xdr::Ed25519Signer::from_seed(&key.to_bytes())
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

#[test]
fn sign_without_identity_uses_configured_default() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_default_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    gen_identity(&dir, "alice");
    set_default_identity(&dir, "alice");
    let unsigned = build_unsigned(&dir);

    // No --identity: the flag's built-in "default" sentinel must resolve to the
    // identity registered with `sdkt identity default`.
    let (ok, stdout, stderr) = run_sign(&dir, &["--input", unsigned.to_str().unwrap()]);
    assert!(
        ok,
        "sign with the configured default identity should succeed, stderr: {stderr}"
    );
    assert!(
        stdout.contains("Signed Transaction Envelope"),
        "stdout: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sign_without_default_identity_suggests_identity_default() {
    let dir = std::env::temp_dir().join(format!("sdkt_sign_nodefault_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // An identity exists, but it is not the default, so the sentinel cannot
    // resolve and the error must point at `sdkt identity default <name>`.
    gen_identity(&dir, "alice");
    let unsigned = build_unsigned(&dir);

    let (ok, _, stderr) = run_sign(&dir, &["--input", unsigned.to_str().unwrap()]);
    assert!(!ok, "sign without a default identity must fail");
    assert!(
        stderr.contains("sdkt identity default"),
        "stderr should suggest setting a default, got: {stderr}"
    );
    // The explicit-name path is unchanged: an unknown name still says so.
    let (ok2, _, stderr2) = run_sign(
        &dir,
        &["--input", unsigned.to_str().unwrap(), "--identity", "ghost"],
    );
    assert!(!ok2, "sign with unknown identity must fail");
    assert!(
        stderr2.contains("unknown identity 'ghost'"),
        "stderr: {stderr2}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sign_with_network_profile_custom_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    gen_identity(dir.path(), "alice");
    let passphrase = "My Custom Passphrase ; 2026";
    add_network_profile(dir.path(), "mynet", "https://rpc.example.com", passphrase);
    let unsigned = build_unsigned(dir.path());
    let signed_path = dir.path().join("signed.xdr");

    let (ok, _, stderr) = run_sign(
        dir.path(),
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--output",
            signed_path.to_str().unwrap(),
            "--identity",
            "alice",
            "--network-profile",
            "mynet",
        ],
    );
    assert!(
        ok,
        "sign with --network-profile should succeed, stderr: {stderr}"
    );

    let signed_b64 = std::fs::read_to_string(&signed_path).unwrap();
    let env = parse_envelope_b64(&signed_b64);
    let signer = load_signer(dir.path(), "alice");

    assert!(
        sdkt_xdr::verify_signature(
            &env,
            &signer,
            &sdkt_xdr::SigningOptions::with(sdkt_xdr::Network::Custom(passphrase.to_string())),
        ),
        "signature must verify against the profile's custom passphrase"
    );
    assert!(
        !sdkt_xdr::verify_signature(
            &env,
            &signer,
            &sdkt_xdr::SigningOptions::with(sdkt_xdr::Network::Testnet),
        ),
        "signature for custom profile must not verify against testnet"
    );
}

#[test]
fn sign_with_network_profile_standard_passphrases_match_network_flag() {
    let dir = tempfile::tempdir().unwrap();
    gen_identity(dir.path(), "alice");
    let unsigned = build_unsigned(dir.path());
    let signer = load_signer(dir.path(), "alice");

    let cases = [
        ("prof-testnet", "testnet", sdkt_xdr::Network::Testnet),
        ("prof-mainnet", "mainnet", sdkt_xdr::Network::Mainnet),
        ("prof-futurenet", "futurenet", sdkt_xdr::Network::Futurenet),
    ];

    for (profile_name, label, expected_net) in cases {
        add_network_profile(
            dir.path(),
            profile_name,
            "https://rpc.example.com",
            expected_net.passphrase(),
        );

        let via_profile = dir.path().join(format!("{profile_name}.xdr"));
        let via_label = dir.path().join(format!("{label}.xdr"));

        let (ok1, _, stderr1) = run_sign(
            dir.path(),
            &[
                "--input",
                unsigned.to_str().unwrap(),
                "--output",
                via_profile.to_str().unwrap(),
                "--identity",
                "alice",
                "--network-profile",
                profile_name,
            ],
        );
        assert!(ok1, "profile sign failed for {profile_name}: {stderr1}");

        let (ok2, _, stderr2) = run_sign(
            dir.path(),
            &[
                "--input",
                unsigned.to_str().unwrap(),
                "--output",
                via_label.to_str().unwrap(),
                "--identity",
                "alice",
                "--network",
                label,
            ],
        );
        assert!(ok2, "label sign failed for {label}: {stderr2}");

        let profile_b64 = std::fs::read_to_string(&via_profile).unwrap();
        let label_b64 = std::fs::read_to_string(&via_label).unwrap();
        assert_eq!(
            profile_b64, label_b64,
            "profile '{profile_name}' must sign identically to --network {label}"
        );

        let env = parse_envelope_b64(&profile_b64);
        assert!(sdkt_xdr::verify_signature(
            &env,
            &signer,
            &sdkt_xdr::SigningOptions::with(expected_net),
        ));
    }
}

#[test]
fn sign_with_network_and_network_profile_mutually_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    gen_identity(dir.path(), "alice");
    add_network_profile(
        dir.path(),
        "mynet",
        "https://rpc.example.com",
        "My Custom Passphrase ; 2026",
    );
    let unsigned = build_unsigned(dir.path());

    // Both flags after `tx sign`
    let (ok, _, stderr) = run_sign(
        dir.path(),
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
    assert!(
        !ok,
        "passing both --network and --network-profile must fail"
    );
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("together"),
        "expected mutual exclusion error, got: {stderr}"
    );

    // Global `--network-profile` before `sign`, `--network` after `sign`
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_IDENTITY_DIR", dir.path())
        .env("SDKT_NETWORK_DIR", dir.path())
        .args([
            "tx",
            "--network-profile",
            "mynet",
            "sign",
            "--input",
            unsigned.to_str().unwrap(),
            "--identity",
            "alice",
            "--network",
            "testnet",
        ])
        .output()
        .expect("failed to run sdkt tx sign");
    assert!(
        !out.status.success(),
        "passing global --network-profile with --network must fail"
    );
    let stderr2 = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr2.contains("cannot be used with") || stderr2.contains("together"),
        "expected mutual exclusion error, got: {stderr2}"
    );
}

#[test]
fn sign_with_unknown_network_profile_errors() {
    let dir = tempfile::tempdir().unwrap();
    gen_identity(dir.path(), "alice");
    let unsigned = build_unsigned(dir.path());

    let (ok, _, stderr) = run_sign(
        dir.path(),
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--identity",
            "alice",
            "--network-profile",
            "ghost",
        ],
    );
    assert!(!ok, "sign with unknown network profile must fail");
    assert!(
        stderr.contains("network profile 'ghost' not found"),
        "expected standard 'not found' error, got: {stderr}"
    );
}

#[test]
fn sign_with_custom_network_label() {
    let dir = tempfile::tempdir().unwrap();
    gen_identity(dir.path(), "alice");
    let unsigned = build_unsigned(dir.path());
    let signed_path = dir.path().join("signed_custom.xdr");

    let (ok, _, stderr) = run_sign(
        dir.path(),
        &[
            "--input",
            unsigned.to_str().unwrap(),
            "--output",
            signed_path.to_str().unwrap(),
            "--identity",
            "alice",
            "--network",
            "custom:my-local-net",
        ],
    );
    assert!(
        ok,
        "sign with --network custom:... should succeed, stderr: {stderr}"
    );

    let signed_b64 = std::fs::read_to_string(&signed_path).unwrap();
    let env = parse_envelope_b64(&signed_b64);
    let signer = load_signer(dir.path(), "alice");
    assert!(sdkt_xdr::verify_signature(
        &env,
        &signer,
        &sdkt_xdr::SigningOptions::with(sdkt_xdr::Network::Custom("my-local-net".to_string())),
    ));
}
