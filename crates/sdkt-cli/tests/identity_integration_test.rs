use assert_cmd::Command;
use tempfile::tempdir;

/// Redirect the keystore for a subprocess to an isolated temp dir.
///
/// `IdentityStore::new()` resolves its location via `ProjectDirs`, which on
/// Linux honors `XDG_CONFIG_HOME`. Setting it per-command (rather than a
/// process-global `env::set_var("HOME", ...)`) keeps the test hermetic and
/// avoids cross-test interference under parallel `cargo test --workspace`.
fn sdkt(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("XDG_CONFIG_HOME", dir);
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
