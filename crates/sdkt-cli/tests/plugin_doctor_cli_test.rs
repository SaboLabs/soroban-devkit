//! Integration tests for `sdkt plugin doctor`.
//!
//! Validates end-to-end plugin diagnostics across all stages:
//! Stage 1: metadata
//! Stage 2: artifact
//! Stage 3: integrity
//! Stage 4: compatibility
//! Stage 5: load
//! Stage 6: self-check
//!
//! Verifies:
//! - Exit codes mapping: 0=healthy, 1..=6 for first failing stage
//! - First-failure-stops execution
//! - JSON schema and output stability
//! - Non-mutating behavior on the plugin store

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn sdkt(store_dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_PLUGIN_DIR", store_dir.path());
    cmd.env("SDKT_NETWORK_DIR", store_dir.path());
    cmd
}

fn make_plugin_dir(root: &Path, id: &str, kind: &str, ext: &str, abi_major: u32) -> PathBuf {
    let src = root.join(id);
    fs::create_dir_all(&src).unwrap();
    let artifact_name = format!("rule.{}", ext);
    fs::write(src.join(&artifact_name), b"dummy-content").unwrap();
    fs::write(
        src.join("plugin.toml"),
        format!(
            r#"id = "{id}"
name = "Test Rule"
version = "1.0.0"
author = "SaboLabs"
description = "Diagnostic test plugin."
kind = "{kind}"
artifact = "{artifact_name}"
abi_major = {abi_major}
abi_minor = 0
"#
        ),
    )
    .unwrap();
    src
}

#[test]
fn doctor_target_not_found() {
    let store = TempDir::new().unwrap();
    sdkt(&store)
        .args(["plugin", "doctor", "non-existent-plugin"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("failed at stage 'metadata'"));

    // JSON check
    let output = sdkt(&store)
        .args([
            "plugin",
            "doctor",
            "non-existent-plugin",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["healthy"], false);
    assert_eq!(json["stages"].as_array().unwrap().len(), 1);
    assert_eq!(json["stages"][0]["name"], "metadata");
    assert_eq!(json["stages"][0]["status"], "failed");
}

#[test]
fn doctor_invalid_metadata_bad_kind() {
    let store = TempDir::new().unwrap();
    let plugin_dir = make_plugin_dir(store.path(), "bad-kind", "unknown-kind", "wasm", 1);

    sdkt(&store)
        .args(["plugin", "doctor", plugin_dir.to_str().unwrap()])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("failed at stage 'metadata'"));

    let output = sdkt(&store)
        .args([
            "plugin",
            "doctor",
            plugin_dir.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["healthy"], false);
    assert_eq!(json["stages"].as_array().unwrap().len(), 1);
    assert_eq!(json["stages"][0]["name"], "metadata");
    assert_eq!(json["stages"][0]["status"], "failed");
    assert!(json["stages"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("kind must be 'native' or 'wasm'"));
}

#[test]
fn doctor_invalid_metadata_reserved_path() {
    let store = TempDir::new().unwrap();
    let plugin_dir = store.path().join("reserved-plugin");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("plugin.toml"),
        r#"id = "reserved-plugin"
name = "Reserved"
version = "1.0.0"
author = "SaboLabs"
description = "d"
kind = "wasm"
artifact = "plugin.toml"
abi_major = 1
abi_minor = 0
"#,
    )
    .unwrap();

    sdkt(&store)
        .args(["plugin", "doctor", plugin_dir.to_str().unwrap()])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("failed at stage 'metadata'"));
}

#[test]
fn doctor_missing_artifact_fails_artifact_stage_with_path() {
    let store = TempDir::new().unwrap();
    let plugin_dir = make_plugin_dir(store.path(), "missing-art", "wasm", "wasm", 1);
    // Delete the artifact file
    fs::remove_file(plugin_dir.join("rule.wasm")).unwrap();

    let output = sdkt(&store)
        .args([
            "plugin",
            "doctor",
            plugin_dir.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2)); // Stage 2 failure
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["healthy"], false);
    let stages = json["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 2); // First failure stops execution
    assert_eq!(stages[0]["name"], "metadata");
    assert_eq!(stages[0]["status"], "passed");
    assert_eq!(stages[1]["name"], "artifact");
    assert_eq!(stages[1]["status"], "failed");
    assert!(stages[1]["detail"].as_str().unwrap().contains("rule.wasm"));
}

#[test]
fn doctor_abi_major_mismatch_fails_compatibility_stage() {
    let store = TempDir::new().unwrap();
    let plugin_dir = make_plugin_dir(store.path(), "bad-abi", "wasm", "wasm", 99);

    let output = sdkt(&store)
        .args([
            "plugin",
            "doctor",
            plugin_dir.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4)); // Stage 4 failure
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["healthy"], false);
    let stages = json["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 4);
    assert_eq!(stages[0]["name"], "metadata");
    assert_eq!(stages[0]["status"], "passed");
    assert_eq!(stages[1]["name"], "artifact");
    assert_eq!(stages[1]["status"], "passed");
    assert_eq!(stages[2]["name"], "integrity");
    assert_eq!(stages[2]["status"], "passed");
    assert_eq!(stages[3]["name"], "compatibility");
    assert_eq!(stages[3]["status"], "failed");
    assert!(stages[3]["detail"]
        .as_str()
        .unwrap()
        .contains("plugin v99.x, host v1.x"));
}

#[test]
fn doctor_hash_tampered_bundle_fails_integrity_stage() {
    let store = TempDir::new().unwrap();
    let tmp = TempDir::new().unwrap();
    let plugin_dir = make_plugin_dir(tmp.path(), "tamper-rule", "wasm", "wasm", 1);

    // Pack bundle
    let bundle_path = tmp.path().join("tamper-rule-1.0.0.sdktplugin");
    sdkt(&store)
        .args([
            "plugin",
            "pack",
            plugin_dir.to_str().unwrap(),
            "--output",
            bundle_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Tamper with the manifest digest in the bundle
    let mut bundle_bytes = fs::read(&bundle_path).unwrap();
    let digest = sdkt_audit::plugin_store::digest_hex(b"dummy-content").into_bytes();
    let offset = bundle_bytes
        .windows(digest.len())
        .position(|window| window == digest)
        .expect("digest must exist in packed bundle");
    bundle_bytes[offset] = if bundle_bytes[offset] == b'0' {
        b'1'
    } else {
        b'0'
    };
    fs::write(&bundle_path, bundle_bytes).unwrap();

    let output = sdkt(&store)
        .args([
            "plugin",
            "doctor",
            bundle_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3)); // Stage 3 failure
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["healthy"], false);
    let stages = json["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 3);
    assert_eq!(stages[0]["name"], "metadata");
    assert_eq!(stages[0]["status"], "passed");
    assert_eq!(stages[1]["name"], "artifact");
    assert_eq!(stages[1]["status"], "passed");
    assert_eq!(stages[2]["name"], "integrity");
    assert_eq!(stages[2]["status"], "failed");
    assert!(stages[2]["detail"]
        .as_str()
        .unwrap()
        .contains("digest mismatch"));
}

#[cfg(feature = "wasm-plugins")]
#[test]
fn doctor_loadable_but_symbol_missing_fails_load_stage() {
    let store = TempDir::new().unwrap();
    let tmp = TempDir::new().unwrap();
    let plugin_dir = tmp.path().join("missing-sym");
    fs::create_dir_all(&plugin_dir).unwrap();

    // A valid empty wasm binary contains the 8-byte wasm header
    // It compiles and instantiates cleanly in Extism/wasmtime, but exports no symbols!
    let valid_empty_wasm = b"\x00asm\x01\x00\x00\x00";
    fs::write(plugin_dir.join("rule.wasm"), valid_empty_wasm).unwrap();
    fs::write(
        plugin_dir.join("plugin.toml"),
        r#"id = "missing-sym"
name = "Missing Symbol"
version = "1.0.0"
author = "SaboLabs"
description = "d"
kind = "wasm"
artifact = "rule.wasm"
abi_major = 1
abi_minor = 0
"#,
    )
    .unwrap();

    let output = sdkt(&store)
        .args([
            "plugin",
            "doctor",
            plugin_dir.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(5)); // Stage 5 failure
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["healthy"], false);
    let stages = json["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 5);
    assert_eq!(stages[0]["name"], "metadata");
    assert_eq!(stages[1]["name"], "artifact");
    assert_eq!(stages[2]["name"], "integrity");
    assert_eq!(stages[3]["name"], "compatibility");
    assert_eq!(stages[4]["name"], "load");
    assert_eq!(stages[4]["status"], "failed");
    assert!(stages[4]["detail"]
        .as_str()
        .unwrap()
        .contains("missing wasm export"));
}

#[cfg(feature = "plugins")]
fn build_native_example_plugin() -> PathBuf {
    let status = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "-p",
            "sdkt-audit-example-rule",
            "--features",
            "plugins",
        ])
        .status()
        .expect("cargo build example plugin");
    assert!(status.success());
    let patterns = [
        "libsdkt_audit_example_rule.so",
        "libsdkt_audit_example_rule.dylib",
        "sdkt_audit_example_rule.dll",
    ];
    for name in patterns {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug")
            .join(name);
        if p.exists() {
            return p;
        }
    }
    panic!("native example plugin cdylib not found");
}

#[cfg(feature = "wasm-plugins")]
fn build_wasm_example_plugin() -> PathBuf {
    let status = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "-p",
            "sdkt-audit-example-rule",
            "--target",
            "wasm32-wasip1",
            "--features",
            "wasm-plugins",
        ])
        .status()
        .expect("cargo build example wasm plugin");
    assert!(status.success());
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-wasip1/debug/sdkt_audit_example_rule.wasm");
    assert!(wasm_path.exists());
    wasm_path
}

#[cfg(feature = "plugins")]
#[test]
fn doctor_healthy_native_plugin_passes_all_stages() {
    let store = TempDir::new().unwrap();
    let tmp = TempDir::new().unwrap();
    let cdylib = build_native_example_plugin();

    let plugin_dir = tmp.path().join("native-rule");
    fs::create_dir_all(&plugin_dir).unwrap();
    let art_name = cdylib.file_name().unwrap().to_str().unwrap();
    fs::copy(&cdylib, plugin_dir.join(art_name)).unwrap();
    fs::write(
        plugin_dir.join("plugin.toml"),
        format!(
            r#"id = "example-native"
name = "Native Rule"
version = "1.0.0"
author = "SaboLabs"
description = "Native example rule"
kind = "native"
artifact = "{art_name}"
abi_major = 1
abi_minor = 0
"#
        ),
    )
    .unwrap();

    // Test direct directory doctor
    let output = sdkt(&store)
        .args([
            "plugin",
            "doctor",
            plugin_dir.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["healthy"], true);
    let stages = json["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 6);
    for stage in stages {
        assert_eq!(stage["status"], "passed");
    }
    assert_eq!(stages[5]["name"], "self-check");
    assert!(stages[5]["detail"]
        .as_str()
        .unwrap()
        .contains("finding(s) emitted"));
}

#[cfg(feature = "wasm-plugins")]
#[test]
fn doctor_healthy_wasm_plugin_passes_all_stages() {
    let store = TempDir::new().unwrap();
    let tmp = TempDir::new().unwrap();
    let wasm_file = build_wasm_example_plugin();

    let plugin_dir = tmp.path().join("wasm-rule");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::copy(&wasm_file, plugin_dir.join("rule.wasm")).unwrap();
    fs::write(
        plugin_dir.join("plugin.toml"),
        r#"id = "example-wasm"
name = "Wasm Rule"
version = "1.0.0"
author = "SaboLabs"
description = "Wasm example rule"
kind = "wasm"
artifact = "rule.wasm"
abi_major = 1
abi_minor = 0
"#,
    )
    .unwrap();

    // Test direct directory doctor
    let output = sdkt(&store)
        .args([
            "plugin",
            "doctor",
            plugin_dir.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["healthy"], true);
    let stages = json["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 6);
    for stage in stages {
        assert_eq!(stage["status"], "passed");
    }
    assert_eq!(stages[5]["name"], "self-check");
    assert!(stages[5]["detail"]
        .as_str()
        .unwrap()
        .contains("finding(s) emitted"));
}

#[test]
fn doctor_does_not_mutate_store() {
    let store = TempDir::new().unwrap();
    let plugin_dir = make_plugin_dir(store.path(), "my-rule", "wasm", "wasm", 1);

    // Read store directory state before running doctor
    let mut files_before = Vec::new();
    for entry in fs::read_dir(&plugin_dir).unwrap() {
        let entry = entry.unwrap();
        let bytes = fs::read(entry.path()).unwrap();
        files_before.push((entry.file_name(), bytes));
    }

    sdkt(&store)
        .args(["plugin", "doctor", "my-rule"])
        .assert()
        .code(predicate::function(|c: &i32| *c == 0 || *c == 5));

    // Verify all files after running doctor are identical
    let mut files_after = Vec::new();
    for entry in fs::read_dir(&plugin_dir).unwrap() {
        let entry = entry.unwrap();
        let bytes = fs::read(entry.path()).unwrap();
        files_after.push((entry.file_name(), bytes));
    }

    assert_eq!(
        files_before, files_after,
        "doctor must not mutate store files"
    );
}

#[test]
fn doctor_all_empty_store() {
    let store = TempDir::new().unwrap();
    sdkt(&store)
        .args(["plugin", "doctor", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No plugins installed"));

    let output = sdkt(&store)
        .args(["plugin", "doctor", "--all", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["healthy"], true);
    assert!(json["reports"].as_array().unwrap().is_empty());
}

#[test]
fn doctor_target_conflicts_with_all() {
    let store = TempDir::new().unwrap();
    sdkt(&store)
        .args(["plugin", "doctor", "some-target", "--all"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn doctor_all_non_empty_store_json_shape() {
    let store = TempDir::new().unwrap();
    make_plugin_dir(store.path(), "test-rule", "wasm", "wasm", 1);

    let output = sdkt(&store)
        .args(["plugin", "doctor", "--all", "--format", "json"])
        .output()
        .unwrap();

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json.get("healthy").is_some());
    let reports = json["reports"]
        .as_array()
        .expect("reports must be an array");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["target"], "test-rule");
    assert!(reports[0]["stages"].as_array().is_some());
}

#[test]
fn doctor_all_installed_does_not_shadow_cwd() {
    let store = TempDir::new().unwrap();
    make_plugin_dir(store.path(), "my-shadow-test", "wasm", "wasm", 1);

    // Create a temporary working directory containing a directory named "my-shadow-test"
    // with invalid metadata (bad toml)
    let cwd_tmp = TempDir::new().unwrap();
    let shadowed_dir = cwd_tmp.path().join("my-shadow-test");
    fs::create_dir_all(&shadowed_dir).unwrap();
    fs::write(
        shadowed_dir.join("plugin.toml"),
        "invalid = toml content [broken",
    )
    .unwrap();

    // Run doctor --all inside cwd_tmp
    // It should check the store-installed plugin (which has valid metadata), NOT the broken cwd directory
    let output = sdkt(&store)
        .current_dir(cwd_tmp.path())
        .args(["plugin", "doctor", "--all", "--format", "json"])
        .output()
        .unwrap();

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let reports = json["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 1);
    // Metadata stage in store plugin should pass!
    let stages = reports[0]["stages"].as_array().unwrap();
    assert_eq!(stages[0]["name"], "metadata");
    assert_eq!(stages[0]["status"], "passed");
}
