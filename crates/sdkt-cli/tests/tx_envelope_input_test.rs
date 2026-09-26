//! `tx validate`, `tx simulate` and `tx submit` resolve `--envelope` through the
//! same helper as `tx sign --input`: a missing file is reported as such before
//! any base64 decoding or RPC call, while existing files and inline base64 keep
//! working.

use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
        .args(args)
        .output()
        .expect("failed to run sdkt");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sdkt_envelope_{}_{}", tag, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build an unsigned transaction envelope on disk using the CLI.
fn build_unsigned(dir: &std::path::Path) -> std::path::PathBuf {
    let unsigned = dir.join("unsigned.xdr");
    let (ok, _, stderr) = run(&[
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
        unsigned.to_str().unwrap(),
    ]);
    assert!(ok, "tx build failed: {}", stderr);
    unsigned
}

fn assert_missing_file_reported(subcommand: &str) {
    let missing = "./no/such/envelope.xdr";
    let (ok, stdout, stderr) = run(&["tx", subcommand, "--envelope", missing]);
    assert!(!ok, "tx {} with a missing file must fail", subcommand);
    assert!(
        stderr.contains("invalid file") && stderr.contains("no such file"),
        "tx {} stderr: {}",
        subcommand,
        stderr
    );
    // The path must not have been decoded as an envelope.
    let combined = format!("{}{}", stdout, stderr).to_lowercase();
    assert!(
        !combined.contains("base64") && !combined.contains("unmarshal"),
        "tx {} decoded the path as an envelope: {}",
        subcommand,
        combined
    );
}

#[test]
fn validate_reports_missing_file() {
    assert_missing_file_reported("validate");
}

#[test]
fn simulate_reports_missing_file() {
    assert_missing_file_reported("simulate");
}

#[test]
fn submit_reports_missing_file() {
    assert_missing_file_reported("submit");
}

#[test]
fn missing_file_error_matches_tx_sign() {
    let missing = "/no/such/file.xdr";
    let (_, _, validate_err) = run(&["tx", "validate", "--envelope", missing]);
    let (_, _, sign_err) = run(&["tx", "sign", "--input", missing, "--identity", "alice"]);
    let expected = format!(
        "Error: invalid file '{}': no such file or directory",
        missing
    );
    assert!(
        validate_err.contains(&expected),
        "validate: {}",
        validate_err
    );
    assert!(sign_err.contains(&expected), "sign: {}", sign_err);
}

#[test]
fn validate_reads_existing_file_and_inline_base64_alike() {
    let dir = temp_dir("validate");
    let path = build_unsigned(&dir);
    let inline = std::fs::read_to_string(&path).unwrap();

    let (file_ok, file_out, file_err) = run(&[
        "tx",
        "validate",
        "--envelope",
        path.to_str().unwrap(),
        "--format",
        "json",
    ]);
    let (inline_ok, inline_out, _) = run(&[
        "tx",
        "validate",
        "--envelope",
        inline.trim(),
        "--format",
        "json",
    ]);

    assert!(file_ok, "validate from file failed: {}", file_err);
    assert!(inline_ok, "validate from inline base64 failed");
    assert_eq!(file_out, inline_out);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inline_base64_containing_slash_is_not_treated_as_a_path() {
    // `/` is a base64 character; such a value must reach the decoder rather than
    // being rejected as a missing file. `validate` is enough here since all
    // three commands share the resolver, and it needs no RPC.
    let (_, stdout, stderr) = run(&["tx", "validate", "--envelope", "AAAA/AAA"]);
    assert!(
        !stderr.contains("invalid file"),
        "inline base64 treated as a path: {}",
        stderr
    );
    assert!(stdout.contains("Validation Report"), "stdout: {}", stdout);
}
