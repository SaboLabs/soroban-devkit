//! Integration tests for `sdkt project add` (issue #544).
//!
//! Fully offline: adding a contract touches only `.sdkt.toml` in the
//! current directory — no network, no identity store.

use assert_cmd::Command;
use predicates::prelude::*;
use sdkt_core::config::DevKitConfig;
use std::path::Path;
use tempfile::TempDir;

fn sdkt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(dir);
    cmd
}

fn contract_dir(project: &Path, name: &str) -> std::path::PathBuf {
    let dir = project.join("contracts").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    dir
}

#[test]
fn add_writes_entry_and_preserves_other_sections() {
    let project = TempDir::new().unwrap();
    let dir = project.path();
    let before = "# Operator comment, must survive\n[network]\nrpc_url = \"http://localhost:8000\"\npassphrase = \"Standalone Network\"\n\n[contracts.token]\npath = \"contracts/token\"\n";
    std::fs::write(dir.join(".sdkt.toml"), before).unwrap();
    contract_dir(dir, "token");
    contract_dir(dir, "vault");

    sdkt(dir)
        .args([
            "project",
            "add",
            "vault",
            "--path",
            "./contracts/vault",
            "--depends-on",
            "token",
            "--ctor-arg",
            "u32:100",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added contract 'vault'"))
        .stdout(predicate::str::contains("sdkt project deploy"));

    let after = std::fs::read_to_string(dir.join(".sdkt.toml")).unwrap();
    assert!(
        after.starts_with(before),
        "existing content must survive byte-for-byte:\n--- before ---\n{before}\n--- after ---\n{after}"
    );

    // The entry loads back via the config loader and is usable by the
    // deploy-order resolver.
    let config = DevKitConfig::from_file(dir.join(".sdkt.toml")).unwrap();
    let vault = config.contracts.get("vault").expect("vault entry present");
    assert_eq!(vault.path, "./contracts/vault");
    assert_eq!(vault.depends_on, vec!["token".to_string()]);
    assert_eq!(vault.ctor_args, vec!["u32:100".to_string()]);
    let order = sdkt_core::project::resolve_deploy_order(&config).unwrap();
    assert_eq!(order, vec!["token".to_string(), "vault".to_string()]);
}

#[test]
fn duplicate_add_fails_and_leaves_file_unchanged() {
    let project = TempDir::new().unwrap();
    let dir = project.path();
    contract_dir(dir, "token");

    sdkt(dir)
        .args(["project", "add", "token", "--path", "./contracts/token"])
        .assert()
        .success();
    let after_first = std::fs::read(dir.join(".sdkt.toml")).unwrap();

    sdkt(dir)
        .args(["project", "add", "token", "--path", "./contracts/token"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));

    assert_eq!(
        std::fs::read(dir.join(".sdkt.toml")).unwrap(),
        after_first,
        "a failed duplicate add must leave .sdkt.toml unchanged"
    );
}

#[test]
fn add_creates_minimal_file_when_none_exists() {
    let project = TempDir::new().unwrap();
    let dir = project.path();
    contract_dir(dir, "token");

    sdkt(dir)
        .args(["project", "add", "token", "--path", "./contracts/token"])
        .assert()
        .success();

    let text = std::fs::read_to_string(dir.join(".sdkt.toml")).unwrap();
    assert!(
        !text.contains("[network]") && !text.contains("[decode]"),
        "a created file must contain only the new entry:\n{text}"
    );
    let config = DevKitConfig::from_file(dir.join(".sdkt.toml")).unwrap();
    assert_eq!(config.contracts.len(), 1);
    assert_eq!(
        config.contracts["token"].path,
        "./contracts/token".to_string()
    );
}

#[test]
fn add_validation_failures_leave_no_file_behind() {
    let project = TempDir::new().unwrap();
    let dir = project.path();
    contract_dir(dir, "token");
    std::fs::create_dir_all(dir.join("contracts").join("nocargo")).unwrap();

    // Bad alias.
    sdkt(dir)
        .args(["project", "add", "bad alias", "--path", "./contracts/token"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid alias"));
    // Missing path.
    sdkt(dir)
        .args(["project", "add", "token", "--path", "./contracts/missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
    // Directory without Cargo.toml.
    sdkt(dir)
        .args(["project", "add", "token", "--path", "./contracts/nocargo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Cargo.toml"));
    // Unknown --depends-on names the alias.
    sdkt(dir)
        .args([
            "project",
            "add",
            "token",
            "--path",
            "./contracts/token",
            "--depends-on",
            "missing-alias",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("missing-alias"));
    // Malformed --ctor-arg.
    sdkt(dir)
        .args([
            "project",
            "add",
            "token",
            "--path",
            "./contracts/token",
            "--ctor-arg",
            "nope",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid --ctor-arg"));

    assert!(
        !dir.join(".sdkt.toml").exists(),
        "no write may happen when any validation fails"
    );
}

#[test]
fn add_json_output_is_one_parsable_object() {
    let project = TempDir::new().unwrap();
    let dir = project.path();
    contract_dir(dir, "token");

    let output = sdkt(dir)
        .args([
            "project",
            "add",
            "token",
            "--path",
            "./contracts/token",
            "--ctor-arg",
            "u32:100",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).expect("JSON output must parse");
    assert_eq!(value["status"], "added");
    assert_eq!(value["alias"], "token");
    assert_eq!(value["path"], "./contracts/token");
    assert_eq!(value["ctor_args"], serde_json::json!(["u32:100"]));
}

#[test]
fn concurrent_adds_both_persist() {
    // Regression test (CodeRabbit review on PR #569): two overlapping
    // `project add` runs used to race on the read-modify-write of
    // `.sdkt.toml`, and the last rename could silently drop the other's
    // entry. The sidecar lock serializes them, so both entries persist.
    let project = TempDir::new().unwrap();
    let dir = project.path();
    contract_dir(dir, "token");
    contract_dir(dir, "vault");

    std::thread::scope(|s| {
        let token = s.spawn(|| {
            sdkt(dir)
                .args(["project", "add", "token", "--path", "./contracts/token"])
                .assert()
                .success();
        });
        let vault = s.spawn(|| {
            sdkt(dir)
                .args(["project", "add", "vault", "--path", "./contracts/vault"])
                .assert()
                .success();
        });
        token.join().unwrap();
        vault.join().unwrap();
    });

    let config = DevKitConfig::from_file(dir.join(".sdkt.toml")).unwrap();
    assert!(
        config.contracts.contains_key("token"),
        "token entry must survive a concurrent add"
    );
    assert!(
        config.contracts.contains_key("vault"),
        "vault entry must survive a concurrent add"
    );
}
