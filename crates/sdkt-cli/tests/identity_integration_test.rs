use assert_cmd::Command;
use predicates::prelude::*;
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
fn test_cli_identity_delete_missing_errors() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["identity", "generate", "alice"])
        .assert()
        .success();

    sdkt(dir.path())
        .args(["identity", "delete", "alice"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Identity 'alice' removed."));

    // A second delete must fail rather than report a removal that did not happen.
    sdkt(dir.path())
        .args(["identity", "delete", "alice"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Identity 'alice' not found"))
        .stdout(predicate::str::contains("removed").not());
}
