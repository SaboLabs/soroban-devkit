use assert_cmd::Command;
use std::env;
use tempfile::tempdir;

#[test]
fn test_cli_identity_lifecycle() {
    let dir = tempdir().unwrap();
    // Use HOME to redirect the ProjectDirs so we don't mess with real ~/.config/sdkt/identities
    env::set_var("HOME", dir.path());

    // 1. Generate
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("identity")
        .arg("generate")
        .arg("alice")
        .assert()
        .success()
        .stdout(predicates::str::contains("generated successfully"));

    // 2. Show
    let mut cmd2 = Command::cargo_bin("sdkt-cli").unwrap();
    cmd2.arg("identity")
        .arg("show")
        .arg("alice")
        .assert()
        .success()
        .stdout(predicates::str::contains("Public Key: G"));

    // 3. List
    let mut cmd3 = Command::cargo_bin("sdkt-cli").unwrap();
    cmd3.arg("identity")
        .arg("list")
        .assert()
        .success()
        .stdout(predicates::str::contains("alice"));

    // 4. Default
    let mut cmd4 = Command::cargo_bin("sdkt-cli").unwrap();
    cmd4.arg("identity")
        .arg("default")
        .arg("alice")
        .assert()
        .success()
        .stdout(predicates::str::contains("set as default"));

    // 5. Delete
    let mut cmd5 = Command::cargo_bin("sdkt-cli").unwrap();
    cmd5.arg("identity")
        .arg("delete")
        .arg("alice")
        .assert()
        .success()
        .stdout(predicates::str::contains("removed"));

    env::remove_var("HOME");
}
