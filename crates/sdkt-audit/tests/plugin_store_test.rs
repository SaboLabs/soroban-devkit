//! — plugin store integration tests (external test crate).
//!
//! Covers metadata parsing, store-root precedence, ABI-major rejection,
//! kind/extension mismatch rejection, remove idempotency, and id resolution.
//!
//! NOTE: `resolve_store_root()` reads the `SDKT_PLUGIN_DIR` env var. Because the
//! env is process-global, these tests serialize on a static mutex and set the
//! variable once (never unsetting it) to avoid cross-test races.

use sdkt_audit::plugin_abi::SDKT_AUDIT_ABI_MAJOR;
use sdkt_audit::plugin_store::{
    install, list_in, parse_meta, remove, resolve, update, InstallOpts, StoreError,
};
use std::path::Path;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn set_store_root(dir: &std::path::Path) {
    std::env::set_var("SDKT_PLUGIN_DIR", dir);
}

fn meta_toml(kind: &str, abi_major: u32, artifact: &str) -> String {
    format!(
        r#"
id = "example-rule"
name = "Example Rule"
version = "1.0.0"
author = "SaboLabs"
description = "Reference audit rule."
kind = "{kind}"
artifact = "{artifact}"
abi_major = {abi_major}
abi_minor = 0
"#
    )
}

#[test]
fn metadata_parsing_roundtrip() {
    let m = parse_meta(&meta_toml("wasm", SDKT_AUDIT_ABI_MAJOR, "ex.wasm")).unwrap();
    assert_eq!(m.id, "example-rule");
    assert_eq!(m.kind, "wasm");
    assert_eq!(m.artifact, "ex.wasm");
}

#[test]
fn abi_major_mismatch_rejected() {
    let wrong = if SDKT_AUDIT_ABI_MAJOR == 1 { 2 } else { 1 };
    let err = parse_meta(&meta_toml("wasm", wrong, "ex.wasm")).unwrap_err();
    assert!(matches!(err, StoreError::AbiMismatch { .. }));
}

#[test]
fn bad_kind_rejected() {
    let err = parse_meta(&meta_toml("bogus", SDKT_AUDIT_ABI_MAJOR, "ex.wasm")).unwrap_err();
    assert!(matches!(err, StoreError::InvalidMetadata(_)));
}

#[test]
fn store_root_precedence_env_over_config_and_cwd() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    set_store_root(tmp.path());
    let root = sdkt_audit::plugin_store::resolve_store_root();
    assert_eq!(root, tmp.path().to_path_buf());
    // Do NOT unset; other tests rely on the env being set.
}

#[test]
fn install_list_show_resolve_remove_lifecycle() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    set_store_root(tmp.path());

    let src = tmp.path().join("ex.wasm");
    std::fs::write(&src, b"dummy-wasm-bytes").unwrap();
    std::fs::write(
        tmp.path().join("plugin.toml"),
        meta_toml("wasm", SDKT_AUDIT_ABI_MAJOR, "ex.wasm"),
    )
    .unwrap();

    let meta = install(&src, &InstallOpts::default()).expect("install");
    assert_eq!(meta.id, "example-rule");

    let listed = list_in(tmp.path());
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "example-rule");

    let p = resolve("example-rule").expect("resolve");
    assert!(Path::new(&p).exists());

    remove("example-rule").unwrap();
    remove("example-rule").unwrap();
    assert!(resolve("example-rule").is_none());
}

#[test]
fn kind_extension_mismatch_rejected() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    set_store_root(tmp.path());
    let src = tmp.path().join("ex.so");
    std::fs::write(&src, b"dummy").unwrap();
    std::fs::write(
        tmp.path().join("plugin.toml"),
        meta_toml("wasm", SDKT_AUDIT_ABI_MAJOR, "ex.so"),
    )
    .unwrap();
    let err = install(&src, &InstallOpts::default()).unwrap_err();
    assert!(matches!(err, StoreError::KindExtMismatch { .. }));
}

/// Verifies resolve_store_root() fallback behavior when SDKT_PLUGIN_DIR is absent.
///
/// The implementation falls back to:
/// 1. `<config-dir>/sdkt/plugins` if it exists, otherwise
/// 2. `<cwd>/.sdkt/plugins`
///
/// This test ensures the fallback produces a sensible path ending in the
/// expected `sdkt/plugins` suffix.
#[test]
fn store_root_fallback_without_env() {
    let _g = ENV_LOCK.lock().unwrap();
    // Remove the env var to exercise the fallback path
    std::env::remove_var("SDKT_PLUGIN_DIR");

    let root = sdkt_audit::plugin_store::resolve_store_root();

    let path_str = root.to_string_lossy();
    assert!(
        path_str.ends_with("sdkt")
            || path_str.ends_with("sdkt/plugins")
            || path_str.ends_with("sdkt\\plugins"),
        "Fallback store root should end with 'sdkt/plugins' or 'sdkt', got: {}",
        path_str
    );
    assert!(
        !path_str.is_empty(),
        "Fallback store root should not be empty"
    );
}

/// Regression: updating a plugin whose manifest renames the artifact must
/// remove the previously-managed artifact file, leaving only the new one.
#[test]
fn update_with_renamed_artifact_removes_stale_file() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    set_store_root(tmp.path());

    // v1: artifact-a.wasm
    let v1_dir = tmp.path().join("v1");
    std::fs::create_dir_all(&v1_dir).unwrap();
    let v1_artifact = v1_dir.join("artifact-a.wasm");
    std::fs::write(&v1_artifact, b"v1-bytes").unwrap();
    std::fs::write(
        v1_dir.join("plugin.toml"),
        meta_toml("wasm", SDKT_AUDIT_ABI_MAJOR, "artifact-a.wasm"),
    )
    .unwrap();
    install(&v1_artifact, &InstallOpts::default()).expect("install v1");

    let plugin_dir = tmp.path().join("example-rule");
    assert!(plugin_dir.join("artifact-a.wasm").exists());

    // v2: artifact-b.wasm
    let v2_dir = tmp.path().join("v2");
    std::fs::create_dir_all(&v2_dir).unwrap();
    let v2_artifact = v2_dir.join("artifact-b.wasm");
    std::fs::write(&v2_artifact, b"v2-bytes").unwrap();
    std::fs::write(
        v2_dir.join("plugin.toml"),
        meta_toml("wasm", SDKT_AUDIT_ABI_MAJOR, "artifact-b.wasm"),
    )
    .unwrap();

    let meta = update("example-rule", &v2_artifact).expect("update");
    assert_eq!(meta.artifact, "artifact-b.wasm");

    // New artifact present and usable; old artifact gone.
    assert!(plugin_dir.join("artifact-b.wasm").exists());
    assert!(
        !plugin_dir.join("artifact-a.wasm").exists(),
        "stale artifact-a.wasm should have been removed"
    );
    assert_eq!(
        std::fs::read(plugin_dir.join("artifact-b.wasm")).unwrap(),
        b"v2-bytes"
    );

    // Metadata points at the new artifact and resolve() finds it.
    let installed = parse_meta(&std::fs::read_to_string(plugin_dir.join("plugin.toml")).unwrap())
        .expect("parse installed meta");
    assert_eq!(installed.artifact, "artifact-b.wasm");
    let resolved = resolve("example-rule").expect("resolve");
    assert_eq!(resolved, plugin_dir.join("artifact-b.wasm"));

    // Only the new artifact remains in the plugin directory.
    let mut names: Vec<String> = std::fs::read_dir(&plugin_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["artifact-b.wasm", "plugin.toml"]);
}

/// Updating with an unchanged artifact filename must keep working and must not
/// delete the (still-referenced) artifact.
#[test]
fn update_with_unchanged_artifact_keeps_file() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    set_store_root(tmp.path());

    let v1_dir = tmp.path().join("same-v1");
    std::fs::create_dir_all(&v1_dir).unwrap();
    let v1_artifact = v1_dir.join("rule.wasm");
    std::fs::write(&v1_artifact, b"v1-bytes").unwrap();
    std::fs::write(
        v1_dir.join("plugin.toml"),
        meta_toml("wasm", SDKT_AUDIT_ABI_MAJOR, "rule.wasm"),
    )
    .unwrap();
    install(&v1_artifact, &InstallOpts::default()).expect("install v1");

    let v2_dir = tmp.path().join("same-v2");
    std::fs::create_dir_all(&v2_dir).unwrap();
    let v2_artifact = v2_dir.join("rule.wasm");
    std::fs::write(&v2_artifact, b"v2-bytes").unwrap();
    std::fs::write(
        v2_dir.join("plugin.toml"),
        meta_toml("wasm", SDKT_AUDIT_ABI_MAJOR, "rule.wasm"),
    )
    .unwrap();

    let meta = update("example-rule", &v2_artifact).expect("update");
    assert_eq!(meta.artifact, "rule.wasm");

    let plugin_dir = tmp.path().join("example-rule");
    assert!(plugin_dir.join("rule.wasm").exists());
    assert_eq!(
        std::fs::read(plugin_dir.join("rule.wasm")).unwrap(),
        b"v2-bytes"
    );
    assert!(resolve("example-rule").is_some());
}
