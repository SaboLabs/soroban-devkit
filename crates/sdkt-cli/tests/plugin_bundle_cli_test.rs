//! — Plugin bundle pack / verify-bundle CLI integration tests.
//!
//! Exercises the end-to-end CLI lifecycle for `.sdktplugin` bundles:
//! `sdkt plugin pack` → `sdkt plugin verify-bundle` → `sdkt plugin install <bundle>` → list.
//!
//! Tests run hermetically via `SDKT_PLUGIN_DIR` (temp store) and never touch the
//! developer's real profile. The dummy artifact is not a real loadable plugin;
//! it only needs to pass metadata + extension validation in `install`.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Absolute path to the workspace root (parent of crates/sdkt-cli).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has parent")
        .parent()
        .expect("crates dir has parent")
        .to_path_buf()
}

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    let dir = TempDir::new().unwrap();
    cmd.env("SDKT_PLUGIN_DIR", dir.path());
    cmd.env("SDKT_NETWORK_DIR", dir.path());
    cmd
}

fn make_plugin_dir(root: &TempDir, kind: &str, ext: &str) -> PathBuf {
    let src = root.path().join("myrule");
    fs::create_dir_all(&src).unwrap();
    let artifact_name = format!("rule.{}", ext);
    fs::write(src.join(&artifact_name), b"placeholder-artifact-content").unwrap();
    fs::write(
        src.join("plugin.toml"),
        format!(
            r#"id = "myrule"
name = "My Rule"
version = "1.0.0"
author = "Test"
description = "A test plugin."
kind = "{}"
artifact = "{}"
abi_major = 1
abi_minor = 0
"#,
            kind, artifact_name
        ),
    )
    .unwrap();
    src
}

fn abi_version() -> u32 {
    // Mirror the host ABI version so metadata validation passes.
    let major = env!("CARGO_PKG_VERSION_MAJOR");
    let _ = major; // abi_major is a constant
    1
}

#[test]
fn bundle_pack_and_verify_roundtrip_unsigned() {
    let root = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let bundle = root.path().join("myrule-1.0.0.sdktplugin");

    // pack
    sdkt()
        .args([
            "plugin",
            "pack",
            src_dir.to_str().unwrap(),
            "--output",
            bundle.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packed plugin"));

    // verify unsigned bundle
    sdkt()
        .args(["plugin", "verify-bundle", bundle.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Bundle is valid"))
        .stdout(predicate::str::contains("signature: UNSIGNED"));
}

#[test]
fn bundle_pack_and_verify_roundtrip_signed() {
    let root = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");

    // Two distinct 32-byte signing seeds. Signing with seed A and verifying with
    // seed B's public key must fail the signature check. Both seeds produce
    // valid (but distinct) Ed25519 verifying keys.
    let secret_a: [u8; 32] = [0xab; 32];
    let secret_b: [u8; 32] = [0xcd; 32];
    let secret_path = root.path().join("secret.key");
    fs::write(&secret_path, secret_a).unwrap();
    // Precompute verifying key for seed B by signing+extracting via the CLI's
    // own dependency is overkill; instead we point --public-key at a file
    // containing seed B's raw 32 bytes. VerifyingKey::from_bytes accepts any
    // 32-byte slice that is a valid compressed point — both seeds qualify.
    let pub_path = root.path().join("public.key");
    fs::write(pub_path.clone(), secret_b).unwrap();

    let bundle = root.path().join("signed.sdktplugin");

    // pack with signing (seed A)
    sdkt()
        .args([
            "plugin",
            "pack",
            src_dir.to_str().unwrap(),
            "--output",
            bundle.to_str().unwrap(),
            "--secret-key",
            secret_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packed plugin"));

    // verify with seed B's public key → signature mismatch
    sdkt()
        .args([
            "plugin",
            "verify-bundle",
            bundle.to_str().unwrap(),
            "--public-key",
            pub_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("signature verification failed"));

    // verify NO public key → signed bundle reports VERIFIED via embedded pubkey
    sdkt()
        .args(["plugin", "verify-bundle", bundle.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("signature: VERIFIED"));
}

/// Parse a command's stdout as JSON, failing with the raw output if it is not.
fn stdout_json(out: &assert_cmd::assert::Assert) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"))
}

#[test]
fn bundle_pack_and_verify_json_unsigned() {
    let root = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let bundle = root.path().join("json.sdktplugin");

    // The "NOT signed" note stays on stderr, so stdout remains pure JSON.
    let packed = sdkt()
        .args([
            "plugin",
            "pack",
            src_dir.to_str().unwrap(),
            "--output",
            bundle.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("NOT signed"));
    let packed = stdout_json(&packed);
    assert_eq!(packed["status"], "packed");
    assert_eq!(packed["output"], bundle.to_str().unwrap());
    assert_eq!(packed["id"], "myrule");
    assert_eq!(packed["version"], "1.0.0");
    assert_eq!(packed["signed"], false);

    let verified = stdout_json(
        &sdkt()
            .args([
                "plugin",
                "verify-bundle",
                bundle.to_str().unwrap(),
                "--format",
                "json",
            ])
            .assert()
            .success(),
    );
    assert_eq!(verified["valid"], true);
    assert_eq!(verified["signed"], false);
    assert_eq!(verified["plugin"]["id"], "myrule");
    assert_eq!(verified["plugin"]["kind"], "wasm");
}

#[test]
fn bundle_pack_and_verify_json_signed_and_mismatch() {
    let root = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let secret_path = root.path().join("secret.key");
    fs::write(&secret_path, [0xab; 32]).unwrap();
    let wrong_pub = root.path().join("public.key");
    fs::write(&wrong_pub, [0xcd; 32]).unwrap();
    let bundle = root.path().join("signed-json.sdktplugin");

    let packed = stdout_json(
        &sdkt()
            .args([
                "plugin",
                "pack",
                src_dir.to_str().unwrap(),
                "--output",
                bundle.to_str().unwrap(),
                "--secret-key",
                secret_path.to_str().unwrap(),
                "--format",
                "json",
            ])
            .assert()
            .success(),
    );
    assert_eq!(packed["signed"], true);

    let verified = stdout_json(
        &sdkt()
            .args([
                "plugin",
                "verify-bundle",
                bundle.to_str().unwrap(),
                "--format",
                "json",
            ])
            .assert()
            .success(),
    );
    assert_eq!(verified["valid"], true);
    assert_eq!(verified["signed"], true);
    assert_eq!(verified["plugin"]["id"], "myrule");

    // A signature mismatch keeps the existing error contract.
    sdkt()
        .args([
            "plugin",
            "verify-bundle",
            bundle.to_str().unwrap(),
            "--public-key",
            wrong_pub.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("signature verification failed"))
        .stdout(predicate::str::is_empty());
}

#[test]
fn bundle_pack_rejects_invalid_format_before_writing() {
    let root = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let bundle = root.path().join("never.sdktplugin");

    sdkt()
        .args([
            "plugin",
            "pack",
            src_dir.to_str().unwrap(),
            "--output",
            bundle.to_str().unwrap(),
            "--format",
            "yaml",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid format 'yaml'"))
        .stdout(predicate::str::is_empty());
    assert!(
        !bundle.exists(),
        "an invalid --format must fail before packing"
    );
}

#[test]
fn bundle_pack_missing_toml_errors() {
    let root = TempDir::new().unwrap();
    let empty = root.path().join("nopom");
    fs::create_dir_all(&empty).unwrap();
    fs::write(empty.join("rule.wasm"), b"x").unwrap();

    sdkt()
        .args(["plugin", "pack", empty.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("plugin.toml not found"));
}

/// `sdkt` bound to an explicit plugin store so state persists across commands.
fn sdkt_in(store: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_PLUGIN_DIR", store);
    cmd.env("SDKT_NETWORK_DIR", store);
    cmd
}

fn pack(src_dir: &std::path::Path, bundle: &std::path::Path, secret: Option<&std::path::Path>) {
    let mut args = vec![
        "plugin".to_string(),
        "pack".into(),
        src_dir.to_str().unwrap().into(),
        "--output".into(),
        bundle.to_str().unwrap().into(),
    ];
    if let Some(secret) = secret {
        args.push("--secret-key".into());
        args.push(secret.to_str().unwrap().into());
    }
    sdkt().args(&args).assert().success();
}

fn installed_ids(store: &std::path::Path) -> Vec<String> {
    let listed = stdout_json(
        &sdkt_in(store)
            .args(["plugin", "list", "--format", "json"])
            .assert()
            .success(),
    );
    listed
        .as_array()
        .expect("plugin list JSON is an array")
        .iter()
        .filter_map(|p| p["id"].as_str().map(String::from))
        .collect()
}

#[test]
fn bundle_install_after_pack_roundtrip() {
    let root = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let bundle = root.path().join("pack-install.sdktplugin");
    pack(&src_dir, &bundle, None);

    sdkt()
        .args(["plugin", "verify-bundle", bundle.to_str().unwrap()])
        .assert()
        .success();

    sdkt_in(store.path())
        .args(["plugin", "install", bundle.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed plugin 'myrule'"))
        .stdout(predicate::str::contains("signature: UNSIGNED"))
        .stderr(predicate::str::contains("NOT signed"));

    assert_eq!(installed_ids(store.path()), vec!["myrule".to_string()]);
    let _ = abi_version(); // touch to confirm constant usage
}

#[test]
fn bundle_install_json_reports_signed_field() {
    let root = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let secret_path = root.path().join("secret.key");
    fs::write(&secret_path, [0xab; 32]).unwrap();

    let unsigned = root.path().join("unsigned.sdktplugin");
    pack(&src_dir, &unsigned, None);
    let store = TempDir::new().unwrap();
    let out = stdout_json(
        &sdkt_in(store.path())
            .args([
                "plugin",
                "install",
                unsigned.to_str().unwrap(),
                "--format",
                "json",
            ])
            .assert()
            .success(),
    );
    assert_eq!(out["status"], "installed");
    assert_eq!(out["signed"], false);
    assert_eq!(out["plugin"]["id"], "myrule");

    let signed = root.path().join("signed.sdktplugin");
    pack(&src_dir, &signed, Some(&secret_path));
    let store = TempDir::new().unwrap();
    let out = stdout_json(
        &sdkt_in(store.path())
            .args([
                "plugin",
                "install",
                signed.to_str().unwrap(),
                "--format",
                "json",
            ])
            .assert()
            .success(),
    );
    assert_eq!(out["status"], "installed");
    assert_eq!(out["signed"], true);
    assert_eq!(installed_ids(store.path()), vec!["myrule".to_string()]);
}

#[test]
fn bundle_install_rejects_tampered_bundle() {
    let root = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let bundle = root.path().join("tampered.sdktplugin");
    pack(&src_dir, &bundle, None);

    // Flip one byte of the artifact payload inside the (uncompressed) tar.
    let mut bytes = fs::read(&bundle).unwrap();
    let needle = b"placeholder-artifact-content";
    let pos = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("artifact payload present in bundle");
    bytes[pos] ^= 0xff;
    fs::write(&bundle, bytes).unwrap();

    sdkt_in(store.path())
        .args(["plugin", "install", bundle.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("digest mismatch"))
        .stdout(predicate::str::is_empty());
    assert!(installed_ids(store.path()).is_empty());
}

#[test]
fn bundle_install_rejects_mismatched_public_key() {
    let root = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let secret_path = root.path().join("secret.key");
    fs::write(&secret_path, [0xab; 32]).unwrap();
    let wrong_pub = root.path().join("public.key");
    fs::write(&wrong_pub, [0xcd; 32]).unwrap();
    let bundle = root.path().join("signed.sdktplugin");
    pack(&src_dir, &bundle, Some(&secret_path));

    sdkt_in(store.path())
        .args([
            "plugin",
            "install",
            bundle.to_str().unwrap(),
            "--public-key",
            wrong_pub.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("signature verification failed"));
    assert!(installed_ids(store.path()).is_empty());
}

#[test]
fn install_public_key_requires_bundle_source() {
    let root = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let key = root.path().join("public.key");
    fs::write(&key, [0xcd; 32]).unwrap();

    sdkt_in(store.path())
        .args([
            "plugin",
            "install",
            src_dir.join("rule.wasm").to_str().unwrap(),
            "--public-key",
            key.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "only applies to .sdktplugin bundles",
        ));
}

#[test]
fn bundle_install_rejects_unsigned_bundle_when_public_key_given() {
    let root = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let key = root.path().join("public.key");
    fs::write(&key, [0xcd; 32]).unwrap();
    let bundle = root.path().join("unsigned.sdktplugin");
    pack(&src_dir, &bundle, None);

    sdkt_in(store.path())
        .args([
            "plugin",
            "install",
            bundle.to_str().unwrap(),
            "--public-key",
            key.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "bundle is not signed but a public key was provided; refusing to install",
        ))
        .stdout(predicate::str::is_empty());
    assert!(installed_ids(store.path()).is_empty());
}

#[test]
fn bundle_install_honors_force_and_id() {
    let root = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let bundle = root.path().join("opts.sdktplugin");
    pack(&src_dir, &bundle, None);
    let install = |extra: &[&str]| {
        let mut cmd = sdkt_in(store.path());
        cmd.args(["plugin", "install", bundle.to_str().unwrap()])
            .args(extra);
        cmd.assert()
    };

    install(&[]).success();
    // Same id again without --force is refused, exactly as for artifacts.
    install(&[])
        .failure()
        .stderr(predicate::str::contains("already installed"));
    install(&["--force"]).success();

    // --id overrides the bundle's metadata id.
    install(&["--id", "renamed"])
        .success()
        .stdout(predicate::str::contains("Installed plugin 'renamed'"));
    let mut ids = installed_ids(store.path());
    ids.sort();
    assert_eq!(ids, vec!["myrule".to_string(), "renamed".to_string()]);
}

#[test]
fn bundle_install_extension_is_case_insensitive() {
    let root = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let src_dir = make_plugin_dir(&root, "wasm", "wasm");
    let bundle = root.path().join("UPPER.SDKTPLUGIN");
    pack(&src_dir, &bundle, None);

    sdkt_in(store.path())
        .args(["plugin", "install", bundle.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("signature: UNSIGNED"));
    assert_eq!(installed_ids(store.path()), vec!["myrule".to_string()]);
}

#[test]
fn m40_deliverable_files_present() {
    // Confirms the deliverables exist: plugin_store.rs, plugin_loader.rs,
    // plugin_cli_test.rs, and plugin_loading.rs.
    let root = workspace_root();
    assert!(
        root.join("crates/sdkt-audit/src/plugin_store.rs").exists(),
        "plugin_store.rs must exist (plugin store)"
    );
    assert!(
        root.join("crates/sdkt-audit/src/plugin_loader.rs").exists(),
        "plugin_loader.rs must exist (plugin store)"
    );
    assert!(
        root.join("crates/sdkt-cli/tests/plugin_cli_test.rs")
            .exists(),
        "plugin_cli_test.rs must exist "
    );
    assert!(
        root.join("crates/sdkt-cli/tests/plugin_loading.rs")
            .exists(),
        "plugin_loading.rs must exist "
    );
}
