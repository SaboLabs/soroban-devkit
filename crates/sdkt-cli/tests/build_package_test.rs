//! Integration tests for `sdkt build --package` (issue #552).
//!
//! Fully offline: a fake `cargo` shim on PATH records every contract it is
//! asked to build and writes a deterministic WASM artifact. That proves which
//! contracts were built and that unselected artifacts were left untouched,
//! without needing a Rust WASM toolchain.

#![cfg(unix)]

use assert_cmd::Command;
use predicates::prelude::*;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const WASM_DIR: &str = "target/wasm32v1-none/release";

fn sdkt(dir: &Path, shim_dir: &Path, log: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(dir);
    let path = format!(
        "{}:{}",
        shim_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    cmd.env("PATH", path);
    cmd.env("SDKT_FAKE_CARGO_LOG", log);
    cmd
}

fn artifact(dir: &Path, alias: &str) -> PathBuf {
    dir.join("contracts")
        .join(alias)
        .join(WASM_DIR)
        .join(format!("{alias}.wasm"))
}

fn hash(path: &Path) -> String {
    sdkt_core::lock::compute_sha256(path).expect("artifact hashable")
}

fn invocation_log(log: &Path) -> Vec<String> {
    match std::fs::read_to_string(log) {
        Ok(contents) => contents.lines().map(str::to_string).collect(),
        Err(_) => Vec::new(),
    }
}

/// Three-contract workspace: token <- vault <- router.
///
/// Each contract's artifact already exists (contents `before:<alias>`), so the
/// test can prove a partial build leaves the unselected ones byte-identical.
fn workspace() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    std::fs::write(
        dir.join(".sdkt.toml"),
        "[contracts.token]\npath = \"contracts/token\"\n\n\
         [contracts.vault]\npath = \"contracts/vault\"\ndepends_on = [\"token\"]\n\n\
         [contracts.router]\npath = \"contracts/router\"\ndepends_on = [\"vault\"]\n",
    )
    .unwrap();

    for alias in ["token", "vault", "router"] {
        let contract = dir.join("contracts").join(alias);
        std::fs::create_dir_all(contract.join("src")).unwrap();
        std::fs::write(
            contract.join("Cargo.toml"),
            format!("[package]\nname = \"{alias}\"\nversion = \"0.1.0\"\n"),
        )
        .unwrap();
        std::fs::write(contract.join("src/lib.rs"), "pub fn noop() {}\n").unwrap();
        let wasm = artifact(&dir, alias);
        std::fs::create_dir_all(wasm.parent().unwrap()).unwrap();
        std::fs::write(&wasm, format!("before:{alias}")).unwrap();
    }

    // Fake `cargo`: record the contract directory it runs in and emit a WASM
    // artifact whose bytes differ from the pre-existing one.
    let shim_dir = dir.join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let shim = shim_dir.join("cargo");
    std::fs::write(
        &shim,
        "#!/bin/sh\n\
         set -e\n\
         name=$(basename \"$PWD\")\n\
         echo \"$name\" >> \"$SDKT_FAKE_CARGO_LOG\"\n\
         mkdir -p target/wasm32v1-none/release\n\
         printf 'built:%s' \"$name\" > \"target/wasm32v1-none/release/$name.wasm\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

    let log = dir.join("cargo-invocations.log");
    (tmp, dir, shim_dir, log)
}

#[test]
fn package_token_builds_only_the_token_artifact() {
    let (_tmp, dir, shim, log) = workspace();
    let before_token = hash(&artifact(&dir, "token"));
    let before_vault = hash(&artifact(&dir, "vault"));
    let before_router = hash(&artifact(&dir, "router"));

    sdkt(&dir, &shim, &log)
        .args(["build", "--package", "token"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Building selected packages (build order): token",
        ))
        .stdout(predicates::str::contains("token ->"))
        .stdout(predicates::str::contains("vault ->").not())
        .stdout(predicates::str::contains("router ->").not());

    assert_eq!(invocation_log(&log), vec!["token".to_string()]);
    assert_ne!(hash(&artifact(&dir, "token")), before_token);
    // Unselected artifacts are byte-identical.
    assert_eq!(hash(&artifact(&dir, "vault")), before_vault);
    assert_eq!(hash(&artifact(&dir, "router")), before_router);
}

#[test]
fn package_router_builds_the_whole_dependency_chain_in_order() {
    let (_tmp, dir, shim, log) = workspace();

    sdkt(&dir, &shim, &log)
        .args(["build", "--package", "router"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Building selected packages (build order): token, vault, router",
        ));

    assert_eq!(
        invocation_log(&log),
        vec![
            "token".to_string(),
            "vault".to_string(),
            "router".to_string()
        ]
    );
}

#[test]
fn package_json_includes_the_selected_set() {
    let (_tmp, dir, shim, log) = workspace();

    let output = sdkt(&dir, &shim, &log)
        .args(["build", "--package", "vault", "--format", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["selected"], serde_json::json!(["token", "vault"]));
    assert_eq!(doc["artifacts"].as_array().unwrap().len(), 2);
}

#[test]
fn unknown_package_fails_before_any_build() {
    let (_tmp, dir, shim, log) = workspace();

    sdkt(&dir, &shim, &log)
        .args(["build", "--package", "missing"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("Unknown package 'missing'"))
        .stderr(predicates::str::contains("token"))
        .stderr(predicates::str::contains("vault"))
        .stderr(predicates::str::contains("router"));

    assert!(
        invocation_log(&log).is_empty(),
        "no build may start for an unknown alias"
    );
}

#[test]
fn partial_build_preserves_unselected_lock_entries() {
    let (_tmp, dir, shim, log) = workspace();

    sdkt(&dir, &shim, &log)
        .args(["lock", "generate"])
        .assert()
        .success();
    let before = sdkt_core::lock::read_lock(&dir).unwrap();
    assert_eq!(before.contracts.len(), 3);
    let vault_before = before
        .contracts
        .iter()
        .find(|entry| entry.alias == "vault")
        .unwrap()
        .clone();
    let router_before = before
        .contracts
        .iter()
        .find(|entry| entry.alias == "router")
        .unwrap()
        .clone();
    let token_before = before
        .contracts
        .iter()
        .find(|entry| entry.alias == "token")
        .unwrap()
        .sha256
        .clone();

    sdkt(&dir, &shim, &log)
        .args(["build", "--package", "token"])
        .assert()
        .success()
        .stderr(predicates::str::contains("partial build"));

    let after = sdkt_core::lock::read_lock(&dir).unwrap();
    let vault_after = after
        .contracts
        .iter()
        .find(|entry| entry.alias == "vault")
        .expect("unselected vault entry preserved");
    let router_after = after
        .contracts
        .iter()
        .find(|entry| entry.alias == "router")
        .expect("unselected router entry preserved");
    assert_eq!(vault_after.sha256, vault_before.sha256);
    assert_eq!(router_after.sha256, router_before.sha256);

    let token_after = &after
        .contracts
        .iter()
        .find(|entry| entry.alias == "token")
        .expect("selected token entry present")
        .sha256;
    assert_ne!(token_after, &token_before, "selected entry is refreshed");
}

#[test]
fn build_without_package_still_builds_everything() {
    let (_tmp, dir, shim, log) = workspace();

    sdkt(&dir, &shim, &log).arg("build").assert().success();

    assert_eq!(
        invocation_log(&log),
        vec![
            "token".to_string(),
            "vault".to_string(),
            "router".to_string()
        ]
    );
}
