//! Integration tests for `sdkt tx validate --verify-signatures` (#563).
//!
//! These exercise the full CLI binary against isolated identity keystores
//! (via a temporary `SDKT_IDENTITY_DIR`) so they never touch the
//! developer's real identities. Envelopes are built and signed with the CLI
//! itself (`tx build` + `tx sign`), except for the hint-collision cases,
//! which use recorded fixtures (see below).

use std::io::Write;
use std::process::Command;

const SRC: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const CONTRACT: &str = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";

/// Run the CLI with an isolated identity/network dir; returns (exit code, stdout, stderr).
fn run_cli(dir: &std::path::Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .env("SDKT_IDENTITY_DIR", dir)
        .env("SDKT_NETWORK_DIR", dir)
        .args(args)
        .output()
        .expect("failed to run sdkt");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Generate an identity and return its G... public key.
fn gen_identity(dir: &std::path::Path, name: &str) -> String {
    let (code, stdout, stderr) = run_cli(dir, &["identity", "generate", name, "--format", "json"]);
    assert_eq!(code, Some(0), "identity generate failed: {}", stderr);
    let v: serde_json::Value =
        serde_json::from_str(&stdout).expect("identity generate must print JSON");
    v["public_key"]
        .as_str()
        .expect("public_key must be a string")
        .to_string()
}

/// Build an unsigned transaction envelope on disk using the CLI.
fn build_unsigned(dir: &std::path::Path) -> std::path::PathBuf {
    let unsigned = dir.join("unsigned.xdr");
    let unsigned_str = unsigned.to_str().unwrap().to_string();
    let (code, _, stderr) = run_cli(
        dir,
        &[
            "tx",
            "build",
            "--source",
            SRC,
            "--sequence",
            "12345",
            "--contract",
            CONTRACT,
            "--function",
            "hello",
            "--output",
            &unsigned_str,
        ],
    );
    assert_eq!(code, Some(0), "tx build failed: {}", stderr);
    unsigned
}

/// Sign an envelope file with a named identity (testnet), writing the result.
fn sign_envelope(
    dir: &std::path::Path,
    input: &std::path::Path,
    output: &std::path::Path,
    identity: &str,
) {
    let (code, _, stderr) = run_cli(
        dir,
        &[
            "tx",
            "sign",
            "--input",
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--identity",
            identity,
            "--network",
            "testnet",
        ],
    );
    assert_eq!(code, Some(0), "tx sign failed: {}", stderr);
}

/// Run `tx validate` on a file; returns (exit code, stdout, stderr).
fn validate(dir: &std::path::Path, args: &[&str]) -> (Option<i32>, String, String) {
    let mut full: Vec<&str> = vec!["tx", "validate"];
    full.extend_from_slice(args);
    run_cli(dir, &full)
}

/// Replace the first signature's bytes with garbage (keeping its hint) and
/// return the resulting envelope as base64. Produces an INVALID signer.
fn corrupt_first_signature_b64(path: &std::path::Path) -> String {
    use base64::Engine;
    use stellar_xdr::{Limited, Limits, ReadXdr, WriteXdr};
    let b64 = std::fs::read_to_string(path).unwrap();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .unwrap();
    let mut cursor = std::io::Cursor::new(&raw);
    let mut l = Limited::new(&mut cursor, Limits::none());
    let mut env = stellar_xdr::TransactionEnvelope::read_xdr(&mut l).unwrap();
    if let stellar_xdr::TransactionEnvelope::Tx(e) = &mut env {
        let mut sigs: Vec<stellar_xdr::DecoratedSignature> = e.signatures.to_vec();
        sigs[0].signature =
            stellar_xdr::Signature(stellar_xdr::BytesM::try_from(vec![0xAAu8; 64]).unwrap());
        e.signatures = stellar_xdr::VecM::try_from(sigs).unwrap();
    } else {
        panic!("expected Tx variant");
    }
    let mut buf = Vec::new();
    let mut l = Limited::new(&mut buf, Limits::none());
    env.write_xdr(&mut l).unwrap();
    base64::engine::general_purpose::STANDARD.encode(&buf)
}

fn write_b64(dir: &std::path::Path, name: &str, b64: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b64.as_bytes()).unwrap();
    path
}

#[test]
fn both_signers_valid_exit_0() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let g2 = gen_identity(dir.path(), "bob");
    let unsigned = build_unsigned(dir.path());
    let signed1 = dir.path().join("signed1.xdr");
    let signed2 = dir.path().join("signed2.xdr");
    sign_envelope(dir.path(), &unsigned, &signed1, "alice");
    sign_envelope(dir.path(), &signed1, &signed2, "bob");

    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            signed2.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
            "--signer",
            &g2,
        ],
    );
    assert_eq!(code, Some(0), "stdout: {}", stdout);
    assert!(stdout.contains("Status: VALID"), "stdout: {}", stdout);
    assert!(stdout.contains("Signatures:"), "stdout: {}", stdout);
    assert!(stdout.contains("Network: testnet"), "stdout: {}", stdout);
    assert!(
        stdout.contains(&format!("VALID {} hint", g1)),
        "stdout: {}",
        stdout
    );
    assert!(
        stdout.contains(&format!("VALID {} hint", g2)),
        "stdout: {}",
        stdout
    );
}

#[test]
fn invalid_signer_via_corrupted_signature_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let unsigned = build_unsigned(dir.path());
    let signed1 = dir.path().join("signed1.xdr");
    sign_envelope(dir.path(), &unsigned, &signed1, "alice");
    let bad_b64 = corrupt_first_signature_b64(&signed1);
    let bad = write_b64(dir.path(), "corrupted.xdr", &bad_b64);

    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            bad.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
        ],
    );
    assert_eq!(code, Some(1), "stdout: {}", stdout);
    assert!(
        stdout.contains(&format!("INVALID {} hint", g1)),
        "stdout: {}",
        stdout
    );
}

#[test]
fn missing_signer_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let g2 = gen_identity(dir.path(), "bob");
    let unsigned = build_unsigned(dir.path());
    let signed1 = dir.path().join("signed1.xdr");
    sign_envelope(dir.path(), &unsigned, &signed1, "alice");

    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            signed1.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
            "--signer",
            &g2,
        ],
    );
    assert_eq!(code, Some(1), "stdout: {}", stdout);
    assert!(
        stdout.contains(&format!("VALID {} hint", g1)),
        "stdout: {}",
        stdout
    );
    assert!(
        stdout.contains(&format!("MISSING {} hint", g2)),
        "stdout: {}",
        stdout
    );
}

#[test]
fn verify_without_signer_is_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let unsigned = build_unsigned(dir.path());
    let (code, _, stderr) = validate(
        dir.path(),
        &[
            "--envelope",
            unsigned.to_str().unwrap(),
            "--verify-signatures",
        ],
    );
    assert_eq!(code, Some(2), "stderr: {}", stderr);
    assert!(stderr.contains("--signer"), "stderr: {}", stderr);
}

#[test]
fn wrong_network_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let g2 = gen_identity(dir.path(), "bob");
    let unsigned = build_unsigned(dir.path());
    let signed1 = dir.path().join("signed1.xdr");
    let signed2 = dir.path().join("signed2.xdr");
    sign_envelope(dir.path(), &unsigned, &signed1, "alice");
    sign_envelope(dir.path(), &signed1, &signed2, "bob");

    // Signed for testnet; verifying under mainnet must not verify.
    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            signed2.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
            "--signer",
            &g2,
            "--network",
            "mainnet",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {}", stdout);
    assert!(stdout.contains("Network: mainnet"), "stdout: {}", stdout);
    assert!(
        stdout.contains(&format!("INVALID {} hint", g1)),
        "stdout: {}",
        stdout
    );
    assert!(
        stdout.contains(&format!("INVALID {} hint", g2)),
        "stdout: {}",
        stdout
    );
}

#[test]
fn duplicate_signer_produces_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let g2 = gen_identity(dir.path(), "bob");
    let unsigned = build_unsigned(dir.path());
    let signed1 = dir.path().join("signed1.xdr");
    let signed2 = dir.path().join("signed2.xdr");
    sign_envelope(dir.path(), &unsigned, &signed1, "alice");
    sign_envelope(dir.path(), &signed1, &signed2, "bob");

    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            signed2.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
            "--signer",
            &g1,
            "--signer",
            &g2,
        ],
    );
    assert_eq!(code, Some(0), "stdout: {}", stdout);
    assert_eq!(
        stdout.matches(&g1).count(),
        1,
        "duplicate signer must produce one line: {}",
        stdout
    );
}

#[test]
fn unmatched_signature_still_exit_0() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let _g2 = gen_identity(dir.path(), "bob");
    let unsigned = build_unsigned(dir.path());
    let signed1 = dir.path().join("signed1.xdr");
    let signed2 = dir.path().join("signed2.xdr");
    sign_envelope(dir.path(), &unsigned, &signed1, "alice");
    sign_envelope(dir.path(), &signed1, &signed2, "bob");

    // Only the first signer is named: the second signature is UNMATCHED
    // (informational) and must not change the exit code.
    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            signed2.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
        ],
    );
    assert_eq!(code, Some(0), "stdout: {}", stdout);
    assert!(stdout.contains("UNMATCHED #1 hint"), "stdout: {}", stdout);
}

#[test]
fn json_output_has_signatures_and_unmatched() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let g2 = gen_identity(dir.path(), "bob");
    let unsigned = build_unsigned(dir.path());
    let signed1 = dir.path().join("signed1.xdr");
    let signed2 = dir.path().join("signed2.xdr");
    sign_envelope(dir.path(), &unsigned, &signed1, "alice");
    sign_envelope(dir.path(), &signed1, &signed2, "bob");

    // All signers named: unmatched is empty.
    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            signed2.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
            "--signer",
            &g2,
            "--format",
            "json",
        ],
    );
    assert_eq!(code, Some(0), "stdout: {}", stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["valid"], true);
    assert_eq!(v["network"], "testnet");
    assert_eq!(v["signatures"].as_array().unwrap().len(), 2);
    assert_eq!(v["signatures"][0]["address"], g1);
    assert_eq!(v["signatures"][0]["status"], "VALID");
    assert_eq!(v["signatures"][0]["candidates"], 1);
    assert!(v["signatures"][0]["hint"].as_str().is_some());
    assert_eq!(v["signatures"][1]["address"], g2);
    assert_eq!(v["unmatched"].as_array().unwrap().len(), 0);

    // Only the first signer named: the second signature is unmatched.
    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            signed2.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
            "--format",
            "json",
        ],
    );
    assert_eq!(code, Some(0), "stdout: {}", stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["signatures"].as_array().unwrap().len(), 1);
    assert_eq!(v["unmatched"].as_array().unwrap().len(), 1);
    assert_eq!(v["unmatched"][0]["index"], 1);
    assert!(v["unmatched"][0]["hint"].as_str().is_some());
}

#[test]
fn offline_with_unreachable_rpc() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let g2 = gen_identity(dir.path(), "bob");
    let unsigned = build_unsigned(dir.path());
    let signed1 = dir.path().join("signed1.xdr");
    let signed2 = dir.path().join("signed2.xdr");
    sign_envelope(dir.path(), &unsigned, &signed1, "alice");
    sign_envelope(dir.path(), &signed1, &signed2, "bob");

    // Verification makes no RPC call: bind a loopback listener and prove
    // the command never connects to it. An unreachable endpoint plus exit
    // 0 would not prove that: a future implementation could attempt a
    // call and fall back to local verification on connection failure.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let rpc_url = format!("http://{}", listener.local_addr().unwrap());
    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            signed2.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
            "--signer",
            &g2,
            "--rpc-url",
            &rpc_url,
        ],
    );
    assert_eq!(code, Some(0), "stdout: {}", stdout);
    // No connection may arrive while the command runs or shortly after:
    // poll the listener briefly, then confirm nothing is queued.
    listener.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    let mut connected = false;
    while std::time::Instant::now() < deadline {
        match listener.accept() {
            Ok(_) => {
                connected = true;
                break;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
    assert!(!connected, "verification must not make an RPC call");
}

#[test]
fn secret_signer_is_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let unsigned = build_unsigned(dir.path());
    // A real S... secret (built from a known seed, never a live key).
    let secret = format!(
        "{}",
        stellar_strkey::Unredacted(&stellar_strkey::ed25519::PrivateKey([0x01; 32]))
    );
    assert!(secret.starts_with('S'));
    let (code, _, stderr) = validate(
        dir.path(),
        &[
            "--envelope",
            unsigned.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &secret,
        ],
    );
    assert_eq!(code, Some(2), "stderr: {}", stderr);
    assert!(
        !stderr.contains(&secret),
        "stderr must not echo the secret: {}",
        stderr
    );
    assert!(
        stderr.contains("expected a G... public key"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn signer_without_verify_is_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let unsigned = build_unsigned(dir.path());
    // A --signer without --verify-signatures is a usage error: the value
    // must not be silently ignored (exit 0 would look like verification).
    // The value itself must not be echoed either.
    let (code, _, stderr) = validate(
        dir.path(),
        &["--envelope", unsigned.to_str().unwrap(), "--signer", &g1],
    );
    assert_eq!(code, Some(2), "stderr: {}", stderr);
    assert!(
        !stderr.contains(&g1),
        "stderr must not echo the signer: {}",
        stderr
    );
}

#[test]
fn structural_only_ignores_missing_network_profile() {
    let dir = tempfile::tempdir().unwrap();
    let unsigned = build_unsigned(dir.path());
    // Structural-only validation must not resolve the network: a missing
    // --network-profile must not stop a valid envelope from validating.
    let (code, stdout, stderr) = validate(
        dir.path(),
        &[
            "--envelope",
            unsigned.to_str().unwrap(),
            "--network-profile",
            "no-such-profile",
        ],
    );
    assert_eq!(code, Some(0), "stdout: {} stderr: {}", stdout, stderr);
}

#[test]
fn structural_failure_has_no_signature_section() {
    let dir = tempfile::tempdir().unwrap();
    let g1 = gen_identity(dir.path(), "alice");
    let unsigned = dir.path().join("lowfee.xdr");
    let unsigned_str = unsigned.to_str().unwrap().to_string();
    let (code, _, stderr) = run_cli(
        dir.path(),
        &[
            "tx",
            "build",
            "--source",
            SRC,
            "--sequence",
            "12345",
            "--contract",
            CONTRACT,
            "--function",
            "hello",
            "--fee",
            "50",
            "--output",
            &unsigned_str,
        ],
    );
    assert_eq!(code, Some(0), "tx build failed: {}", stderr);
    let signed = dir.path().join("lowfee_signed.xdr");
    sign_envelope(dir.path(), &unsigned, &signed, "alice");

    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            signed.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            &g1,
        ],
    );
    assert_eq!(code, Some(1), "stdout: {}", stdout);
    assert!(stdout.contains("Status: INVALID"), "stdout: {}", stdout);
    assert!(
        !stdout.contains("Signatures:"),
        "structural failure must have no signature section: {}",
        stdout
    );
}

// Hint-collision fixtures (recorded):
// Two public keys whose last four bytes are identical (hint 7717b798),
// found by a birthday search over sequential seeds: seed n is 32 zero bytes
// with the final 8 bytes set to n as big-endian u64. Seeds 160866 and
// 172789 collide after 172,789 keys tried. The two-signature envelope was
// signed by both keys (testnet); the one-signature envelope by the first
// key only. No secret material is recorded: only the G... addresses and the
// signed envelopes (public data).
const COLLISION_A: &str = "GDBGXGHXNNBJW3PPLADIP6TFIWIQC5XWBEPCROS5JKTTLOTXC63ZRUWO";
const COLLISION_B: &str = "GDI4E7AOZLSBHUELV5XYBL65VFW54YHYKSKTPFM5DGB23LDXC63ZQIKX";

fn collision_fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn collision_both_signers_valid() {
    let dir = tempfile::tempdir().unwrap();
    let two = collision_fixture("tx_validate_collision_two_sigs.b64");
    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            two.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            COLLISION_A,
            "--signer",
            COLLISION_B,
        ],
    );
    assert_eq!(code, Some(0), "stdout: {}", stdout);
    assert!(
        stdout.contains(&format!(
            "VALID {} hint 7717b798 (2 candidates)",
            COLLISION_A
        )),
        "stdout: {}",
        stdout
    );
    assert!(
        stdout.contains(&format!(
            "VALID {} hint 7717b798 (2 candidates)",
            COLLISION_B
        )),
        "stdout: {}",
        stdout
    );
}

#[test]
fn collision_one_signature_valid_and_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let one = collision_fixture("tx_validate_collision_one_sig.b64");
    let (code, stdout, _) = validate(
        dir.path(),
        &[
            "--envelope",
            one.to_str().unwrap(),
            "--verify-signatures",
            "--signer",
            COLLISION_A,
            "--signer",
            COLLISION_B,
        ],
    );
    assert_eq!(code, Some(1), "stdout: {}", stdout);
    assert!(
        stdout.contains(&format!(
            "VALID {} hint 7717b798 (1 candidate)",
            COLLISION_A
        )),
        "stdout: {}",
        stdout
    );
    assert!(
        stdout.contains(&format!(
            "INVALID {} hint 7717b798 (1 candidate)",
            COLLISION_B
        )),
        "stdout: {}",
        stdout
    );
}
