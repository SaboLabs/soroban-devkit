//! Integration tests for the `sdkt network` CLI command group (M28.2).
//!
//! Each test isolates the network store by pointing `SDKT_NETWORK_DIR` at a
//! temporary directory. `NetworkStore::new()` honors this override, so the
//! on-disk default config directory is never touched.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

/// Build a `sdkt` command with `SDKT_NETWORK_DIR` pointing at `dir`.
fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_NETWORK_DIR", dir);
    cmd
}

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

    // File should exist on disk.
    assert!(dir.path().join("solo.json").exists());

    sdkt(dir.path())
        .args(["network", "remove", "solo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Network profile 'solo' removed."));

    assert!(!dir.path().join("solo.json").exists());

    // Removing again should fail.
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

    // Only one profile file should exist (overwrite, not duplicate).
    let count = fs::read_dir(dir.path())
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
