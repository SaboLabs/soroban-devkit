//! Integration tests for automatic installed plugin execution during `sdkt audit` (#80).
//!
//! Verifies:
//! 1. Installed plugins in the plugin store run during `sdkt audit` without passing `--rules`.
//! 2. `sdkt audit --rules <id>` still resolves and runs that specific rule explicitly.
//! 3. Plugins with incompatible ABI major versions produce a clear warning and are skipped without failing the audit.
//! 4. Audits with zero installed plugins (or with `--no-plugins`) behave identically to the baseline output.
//! 5. The audit report displays a rules-loaded summary line when plugin rules are loaded.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").expect("sdkt binary built")
}

fn write_contract_source(dir: &Path, content: &str) -> PathBuf {
    let p = dir.join("contract.rs");
    fs::write(&p, content).expect("failed to write contract source");
    p
}

/// Helper to set up an incompatible-ABI fixture plugin directly in the store.
fn install_incompatible_abi_fixture(store: &Path, plugin_id: &str, bad_major: u32) {
    let pdir = store.join(plugin_id);
    fs::create_dir_all(&pdir).unwrap();
    let artifact_name = "dummy.so";
    fs::write(pdir.join(artifact_name), b"dummy").unwrap();
    fs::write(
        pdir.join("plugin.toml"),
        format!(
            r#"id = "{plugin_id}"
name = "Incompatible Rule"
version = "1.0.0"
author = "SaboLabs"
description = "Rule with incompatible ABI."
kind = "native"
artifact = "{artifact_name}"
abi_major = {bad_major}
abi_minor = 0
"#
        ),
    )
    .unwrap();
}

#[test]
fn test_zero_installed_plugins_output_unchanged() {
    let temp = TempDir::new().unwrap();
    let store = temp.path().join("store");
    fs::create_dir_all(&store).unwrap();

    let contract = write_contract_source(
        temp.path(),
        "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n",
    );

    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .env("SDKT_NETWORK_DIR", temp.path())
        .args(["audit", contract.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rules loaded:").not())
        .stdout(predicate::str::contains("Static Analysis Report:"))
        .stdout(predicate::str::contains(
            "Severity: 0 critical, 0 warning, 0 info (0 total)",
        ))
        .stdout(predicate::str::contains("No issues found."));
}

#[test]
fn test_incompatible_abi_plugin_warning_and_skip_without_rules() {
    let temp = TempDir::new().unwrap();
    let store = temp.path().join("store");
    fs::create_dir_all(&store).unwrap();

    install_incompatible_abi_fixture(&store, "incompatible-rule", 99);

    let contract = write_contract_source(
        temp.path(),
        "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n",
    );

    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .env("SDKT_NETWORK_DIR", temp.path())
        .args(["audit", contract.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Warning: skipping plugin 'incompatible-rule': ABI mismatch (plugin v99.x, host v1.x)",
        ))
        .stdout(predicate::str::contains("Rules loaded:").not())
        .stdout(predicate::str::contains("No issues found."));
}

#[test]
fn test_incompatible_abi_plugin_warning_and_skip_with_rules_flag() {
    let temp = TempDir::new().unwrap();
    let store = temp.path().join("store");
    fs::create_dir_all(&store).unwrap();

    install_incompatible_abi_fixture(&store, "incompatible-rule", 99);

    let contract = write_contract_source(
        temp.path(),
        "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n",
    );

    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .env("SDKT_NETWORK_DIR", temp.path())
        .args([
            "audit",
            contract.to_str().unwrap(),
            "--rules",
            "incompatible-rule",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Warning: skipping plugin 'incompatible-rule': ABI mismatch (plugin v99.x, host v1.x)",
        ))
        .stdout(predicate::str::contains("Rules loaded:").not())
        .stdout(predicate::str::contains("No issues found."));
}

#[cfg(feature = "plugins")]
fn find_or_build_example_cdylib() -> PathBuf {
    let status = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "-p",
            "sdkt-audit-example-rule",
            "--features",
            "plugins",
        ])
        .status()
        .expect("cargo build for sdkt-audit-example-rule failed");
    assert!(status.success(), "example rule build failed");

    let cdylib_names = [
        "libsdkt_audit_example_rule.so",
        "sdkt_audit_example_rule.dll",
        "libsdkt_audit_example_rule.dylib",
    ];
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for dir in [
        manifest_dir.join("../../target/debug"),
        manifest_dir.join("../../target/debug/deps"),
    ] {
        for name in cdylib_names {
            let p = dir.join(name);
            if p.exists() {
                return p;
            }
        }
    }
    panic!("compiled cdylib for sdkt-audit-example-rule not found in target/debug");
}

#[cfg(feature = "plugins")]
#[test]
fn test_installed_plugin_runs_without_rules_flag() {
    let temp = TempDir::new().unwrap();
    let store = temp.path().join("store");
    fs::create_dir_all(&store).unwrap();

    let cdylib = find_or_build_example_cdylib();
    let ext = cdylib.extension().unwrap().to_str().unwrap();

    // Prepare plugin source dir to install from
    let p_src = temp.path().join("src_plugin");
    fs::create_dir_all(&p_src).unwrap();
    let artifact_dest = p_src.join(format!("my_rule.{ext}"));
    fs::copy(&cdylib, &artifact_dest).unwrap();
    fs::write(
        p_src.join("plugin.toml"),
        format!(
            r#"id = "example-rule"
name = "Example Rule"
version = "1.0.0"
author = "SaboLabs"
description = "Reference audit rule."
kind = "native"
artifact = "my_rule.{ext}"
abi_major = 1
abi_minor = 0
"#
        ),
    )
    .unwrap();

    // Install the plugin into the temp store
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .env("SDKT_NETWORK_DIR", temp.path())
        .args(["plugin", "install", artifact_dest.to_str().unwrap()])
        .assert()
        .success();

    let contract = write_contract_source(
        temp.path(),
        "pub fn sdkt_example_trigger_admin() { require_auth(); }\n",
    );

    // 1. Audit without --rules: plugin runs automatically!
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .env("SDKT_NETWORK_DIR", temp.path())
        .args(["audit", contract.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Rules loaded: 5 built-in, 1 plugin",
        ))
        .stdout(predicate::str::contains("EXAMPLE-001"))
        .stdout(predicate::str::contains("sdkt_example_trigger_admin"));

    // 2. Audit with explicit --rules <id>: still resolves and runs specific rule!
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .env("SDKT_NETWORK_DIR", temp.path())
        .args([
            "audit",
            contract.to_str().unwrap(),
            "--rules",
            "example-rule",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Rules loaded: 5 built-in, 1 plugin",
        ))
        .stdout(predicate::str::contains("EXAMPLE-001"));

    // 3. Audit with --no-plugins: skips auto-loading installed plugins!
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .env("SDKT_NETWORK_DIR", temp.path())
        .args(["audit", contract.to_str().unwrap(), "--no-plugins"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rules loaded:").not());
}
