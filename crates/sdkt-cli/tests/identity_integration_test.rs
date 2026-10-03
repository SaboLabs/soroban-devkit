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

/// Write a throwaway secret to a temp file and return its path.
fn throwaway_secret_at(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, "s3cr3t-bytes-not-a-real-key").unwrap();
    path
}

#[test]
fn test_cli_identity_import_argv_warns_but_succeeds() {
    let dir = tempdir().unwrap();
    let secret_path = throwaway_secret_at(dir.path(), "secret.txt");
    let secret = std::fs::read_to_string(&secret_path).unwrap();

    sdkt(dir.path())
        .args(["identity", "import", "alice", &secret])
        .assert()
        .success()
        .stderr(predicate::str::contains("deprecated"))
        .stdout(predicate::str::contains("imported successfully"));
}

#[test]
fn test_cli_identity_import_stdin_without_secret_arg() {
    let dir = tempdir().unwrap();
    let secret_path = throwaway_secret_at(dir.path(), "secret.txt");
    let secret = std::fs::read_to_string(&secret_path).unwrap();

    sdkt(dir.path())
        .args(["identity", "import", "bob", "-"])
        .write_stdin(secret)
        .assert()
        .success()
        .stdout(predicate::str::contains("imported successfully"));
}

#[test]
fn test_cli_identity_import_empty_stdin_errors() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["identity", "import", "carol", "-"])
        .write_stdin("")
        .assert()
        .failure()
        .stderr(predicate::str::contains("No secret provided"));
}