//! M39 — Release Polish & SCF Readiness integration tests.
//!
//! Covers:
//! 1. `sdkt --version` shape (plain build has no provenance; provenance only
//!    appears when the `provenance` feature is compiled in).
//! 2. Mutating commands refuse an unsafe mainnet configuration with a clear,
//!    actionable error (reusing the existing M29 network resolution + the new
//!    `sdkt_core::guard_mutating_network` guard).
//! 3. M39 deliverable files exist at the workspace root.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

/// Absolute path to the workspace root (parent of the crate manifest dir).
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR for this integration test is crates/sdkt-cli.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has parent")
        .parent()
        .expect("crates dir has parent")
        .to_path_buf()
}

#[test]
fn version_is_semver_without_provenance() {
    // Default build: --version must be exactly the semantic version (no commit/date).
    sdkt()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("2.4.0"))
        .stdout(predicate::function(|s: &str| {
            // No provenance suffix leaked into the default build.
            !s.contains("commit") && !s.contains("built")
        }));
}

#[test]
fn mutating_submit_refuses_mainnet_rpc_with_testnet_passphrase() {
    // Point at mainnet RPC but keep the default testnet passphrase. The guard
    // must reject before any network call (bogus envelope => only the guard
    // path is exercised, since the guard runs first).
    sdkt()
        .args([
            "tx",
            "--rpc-url",
            "https://soroban-rpc.stellar.org",
            "submit",
            "--envelope",
            "AAAA",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not the mainnet passphrase"));
}

#[test]
fn mutating_deploy_refuses_mainnet_rpc_with_testnet_passphrase() {
    sdkt()
        .args([
            "deploy",
            "--wasm",
            "nonexistent.wasm",
            "--salt",
            "salt123",
            "--rpc-url",
            "https://soroban-rpc.stellar.org",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not the mainnet passphrase"));
}

#[test]
fn mutating_submit_allows_testnet_default() {
    // Default network is testnet; with no explicit network the guard passes.
    // The command should then fail for a different, expected reason (bad
    // envelope / network) — not the mainnet-safety guard.
    let out = sdkt()
        .args(["tx", "submit", "--envelope", "AAAA"])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !combined.contains("not the mainnet passphrase")
            && !combined.contains("not explicitly selected"),
        "default testnet submit must not trip the mainnet-safety guard"
    );
}

#[test]
fn mutating_submit_allows_explicit_mainnet_with_matching_passphrase() {
    // Explicitly selecting mainnet with the matching passphrase is permitted by
    // the guard (the command may still fail downstream for other reasons, but
    // not due to the safety guard).
    let out = sdkt()
        .args([
            "tx",
            "--rpc-url",
            "https://soroban-rpc.stellar.org",
            "--network-passphrase",
            "Public Global Stellar Network ; September 2015",
            "submit",
            "--envelope",
            "AAAA",
        ])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !combined.contains("not the mainnet passphrase")
            && !combined.contains("not explicitly selected"),
        "explicit mainnet with matching passphrase must pass the safety guard"
    );
}

#[test]
fn m39_deliverable_files_present() {
    // The M39 deliverables include a Dockerfile + .dockerignore and docs/scf.md.
    let root = workspace_root();
    assert!(
        root.join("Dockerfile").exists(),
        "Dockerfile must exist (M39 deliverable)"
    );
    assert!(
        root.join(".dockerignore").exists(),
        ".dockerignore must exist (M39 deliverable)"
    );
    assert!(
        root.join("docs/scf.md").exists(),
        "docs/scf.md must exist (M39 deliverable)"
    );
}
