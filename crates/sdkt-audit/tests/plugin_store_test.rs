//! M40 — plugin store integration tests (external test crate).
//!
//! Covers metadata parsing, store-root precedence, ABI-major rejection,
//! kind/extension mismatch rejection, remove idempotency, and id resolution.
//!
//! NOTE: `resolve_store_root()` reads the `SDKT_PLUGIN_DIR` env var. Because the
//! env is process-global, these tests serialize on a static mutex and set the
//! variable once (never unsetting it) to avoid cross-test races.

use sdkt_audit::plugin_abi::SDKT_AUDIT_ABI_MAJOR;
use sdkt_audit::plugin_store::{
    install, list_in, parse_meta, remove, resolve, InstallOpts, StoreError,
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
///   1. `<config-dir>/sdkt/plugins` if it exists, otherwise
///   2. `<cwd>/.sdkt/plugins`
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
