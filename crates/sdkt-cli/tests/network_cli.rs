//! Integration tests for `sdkt network` and M29 network-profile resolution.
//!
//! Every test here is CI-safe:
//! - No test depends on a locally running RPC server.
//! - No test depends on internet access.
//! - No test depends on machine-specific configuration.
//!
//! The pure precedence logic (flags > profile > .sdkt.toml > defaults) is
//! covered by unit tests in `main.rs` (`resolver_tests`), which need no I/O.
//! These integration tests cover the CLI surface and the one deterministic
//! end-to-end path: a missing profile is rejected *before* any network call.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

/// Build a `sdkt` command with `SDKT_NETWORK_DIR` pointing at `dir`.
fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_NETWORK_DIR", dir);
    cmd
}

// ---------- M28.2: `sdkt network` management ----------

#[test]
fn network_add_then_list_pretty() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "testnet",
            "--rpc-url",
            "https://soroban-testnet.stellar.org",
            "--passphrase",
            "Test SDF Network ; September 2015",
            "--friendbot",
            "https://friendbot.stellar.org",
            "--description",
            "Stellar testnet",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Network profile 'testnet' saved."));

    sdkt(dir.path())
        .args(["network", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("testnet"))
        .stdout(predicate::str::contains(
            "https://soroban-testnet.stellar.org",
        ));
}

#[test]
fn network_add_then_show_json() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "mainnet",
            "--rpc-url",
            "https://soroban-mainnet.stellar.org",
            "--passphrase",
            "Public Global Stellar Network ; September 2015",
        ])
        .assert()
        .success();

    sdkt(dir.path())
        .args(["network", "show", "mainnet", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"mainnet\""))
        .stdout(predicate::str::contains(
            "Public Global Stellar Network ; September 2015",
        ));
}

#[test]
fn network_show_missing_errors() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["network", "show", "ghost"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not found").not())
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn network_remove_deletes_profile() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "solo",
            "--rpc-url",
            "https://solo.example",
            "--passphrase",
            "Solo Passphrase",
        ])
        .assert()
        .success();

    assert!(dir.path().join("solo.json").exists());

    sdkt(dir.path())
        .args(["network", "remove", "solo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Network profile 'solo' removed."));

    assert!(!dir.path().join("solo.json").exists());

    sdkt(dir.path())
        .args(["network", "remove", "solo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn network_add_overwrites_existing() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "net",
            "--rpc-url",
            "https://old.example",
            "--passphrase",
            "Old",
        ])
        .assert()
        .success();

    sdkt(dir.path())
        .args([
            "network",
            "add",
            "net",
            "--rpc-url",
            "https://new.example",
            "--passphrase",
            "New",
        ])
        .assert()
        .success();

    let count = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .path()
                .extension()
                .and_then(|s| s.to_str())
                == Some("json")
        })
        .count();
    assert_eq!(count, 1);

    sdkt(dir.path())
        .args(["network", "show", "net", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("https://new.example"));
}

#[test]
fn network_list_empty_message() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["network", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No network profiles found."));
}

// ---------- M29: network-profile resolution (CLI surface, CI-safe) ----------

#[test]
fn network_profile_not_found_fails_before_rpc() {
    let dir = tempdir().unwrap();

    // A profile that does not exist must be rejected at resolution time,
    // before any RPC call is attempted. This is deterministic: it touches only
    // the local network store (via SDKT_NETWORK_DIR) and never reaches the network.
    sdkt(dir.path())
        .args(["account", "GABC", "--network-profile", "ghost"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn rpc_commands_expose_network_profile_flag() {
    // Backward compatibility: the flag is present on RPC commands, and the
    // help output still parses (existing interface unchanged). No network used.
    for cmd in ["inspect", "account", "events", "health", "verify", "deploy"] {
        sdkt(std::path::Path::new("/dev/null"))
            .args([cmd, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--network-profile"));
    }
}

#[test]
fn existing_commands_work_without_profiles() {
    let dir = tempdir().unwrap();

    // Commands without --network-profile must still parse and behave exactly
    // as before. `sdkt build --help` is a non-RPC command that must succeed
    // offline; `sdkt network list` is the profile manager itself.
    sdkt(dir.path())
        .args(["build", "--help"])
        .assert()
        .success();

    sdkt(dir.path())
        .args(["network", "list"])
        .assert()
        .success();
}
