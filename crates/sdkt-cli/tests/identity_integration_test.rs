use assert_cmd::Command;
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
fn test_cli_identity_import_from_stdin() {
    let dir = tempdir().unwrap();
    let secret = "SAAACAQDAQCQMBYIBEFAWDANBYHRAEISCMKBKFQXDAMRUGY4DUPB6NKI";

    sdkt(dir.path())
        .args(["identity", "import", "bob"])
        .write_stdin(secret)
        .assert()
        .success()
        .stdout(predicates::str::contains("imported successfully"))
        .stdout(predicates::str::contains("Public Key: G"));

    sdkt(dir.path())
        .args(["identity", "show", "bob"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Public Key: G"));
}

#[test]
fn test_cli_identity_import_rejects_empty_stdin() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["identity", "import", "bob"])
        .write_stdin("")
        .assert()
        .failure();
}

#[test]
fn test_cli_identity_import_rejects_invalid_secret() {
    let dir = tempdir().unwrap();

    sdkt(dir.path())
        .args(["identity", "import", "bob"])
        .write_stdin("not-a-valid-secret-key")
        .assert()
        .failure();
}

#[test]
fn test_cli_identity_import_no_longer_accepts_argv_secret() {
    let dir = tempdir().unwrap();
    let secret = "SAAACAQDAQCQMBYIBEFAWDANBYHRAEISCMKBKFQXDAMRUGY4DUPB6NKI";

    // Positional argv secret must be rejected (extra arg → clap usage error).
    sdkt(dir.path())
        .args(["identity", "import", "bob", secret])
        .assert()
        .failure();
}
