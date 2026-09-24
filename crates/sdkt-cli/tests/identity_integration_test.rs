use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::tempdir;

/// Redirect the keystore for a subprocess to an isolated temp dir.
///
/// Uses `SDKT_IDENTITY_DIR` (checked first by `IdentityStore::new()`) rather
/// than platform-specific vars like `XDG_CONFIG_HOME` / `APPDATA` / `HOME`.
/// This keeps the test hermetic and cross-platform on Linux, macOS, and Windows.
fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd
}

#[test]
fn test_cli_identity_lifecycle() {
    let dir = tempdir().unwrap();

    // 1. Generate
    sdkt(dir.path())
        .args(["identity", "generate", "alice"])
        .assert()
        .success()
        .stdout(predicates::str::contains("generated successfully"));

    // 2. Show
    sdkt(dir.path())
        .args(["identity", "show", "alice"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Public Key: G"));

    // 3. List
    sdkt(dir.path())
        .args(["identity", "list"])
        .assert()
        .success()
        .stdout(predicates::str::contains("alice"));

    // 4. Default
    sdkt(dir.path())
        .args(["identity", "default", "alice"])
        .assert()
        .success()
        .stdout(predicates::str::contains("set as default"));

    // 5. Delete
    sdkt(dir.path())
        .args(["identity", "delete", "alice"])
        .assert()
        .success()
        .stdout(predicates::str::contains("removed"));
}

#[test]
fn identity_generate_format_json() {
    let dir = tempdir().unwrap();

    let output = sdkt(dir.path())
        .args(["identity", "generate", "bob", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(v["name"], "bob");
    assert!(
        v["public_key"].as_str().unwrap_or("").starts_with('G'),
        "public_key should be a G... address"
    );
    // No secret material
    assert!(v.get("secret_key").is_none());
    assert!(v.get("secret").is_none());
    // Pretty prose must not appear
    let raw = String::from_utf8_lossy(&output);
    assert!(!raw.contains("generated successfully"));
}

#[test]
fn identity_list_format_json_includes_default_marker() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["identity", "generate", "alice"])
        .assert()
        .success();
    sdkt(dir.path())
        .args(["identity", "generate", "bob"])
        .assert()
        .success();
    sdkt(dir.path())
        .args(["identity", "default", "bob"])
        .assert()
        .success();

    let output = sdkt(dir.path())
        .args(["identity", "list", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let items: Vec<Value> = serde_json::from_slice(&output).expect("JSON array");
    assert_eq!(items.len(), 2);

    let alice = items.iter().find(|i| i["name"] == "alice").unwrap();
    let bob = items.iter().find(|i| i["name"] == "bob").unwrap();
    assert_eq!(alice["default"], false);
    assert_eq!(bob["default"], true);
    assert!(alice["public_key"].as_str().unwrap().starts_with('G'));
    assert!(alice.get("secret_key").is_none());
}

#[test]
fn identity_show_format_json() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["identity", "generate", "carol"])
        .assert()
        .success();
    let output = sdkt(dir.path())
        .args(["identity", "show", "carol", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(v["name"], "carol");
    assert!(v["public_key"].as_str().unwrap().starts_with('G'));
    assert!(v.get("secret_key").is_none());
    // Show JSON mirrors pretty fields only (no invented default marker).
    assert!(v.get("default").is_none());

    let raw = String::from_utf8_lossy(&output);
    assert!(!raw.contains("Public Key:"));
}

#[test]
fn identity_pretty_output_unchanged_by_default() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["identity", "generate", "dave"])
        .assert()
        .success()
        .stdout(predicate::str::contains("generated successfully"))
        .stdout(predicate::str::contains("Public Key: G"));

    sdkt(dir.path())
        .args(["identity", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Identities:"))
        .stdout(predicate::str::contains("dave"));

    sdkt(dir.path())
        .args(["identity", "show", "dave"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Identity: dave"))
        .stdout(predicate::str::contains("Public Key: G"));
}

#[test]
fn identity_list_json_empty_is_array() {
    let dir = tempdir().unwrap();

    let output = sdkt(dir.path())
        .args(["identity", "list", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let items: Vec<Value> = serde_json::from_slice(&output).expect("JSON array");
    assert!(items.is_empty());
}
