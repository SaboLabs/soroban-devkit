use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::process::Command as StdCommand;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

#[test]
fn init_full_creates_all_files() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("hello");

    sdkt()
        .args(["init", project.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project"))
        .stdout(predicate::str::contains("Ready to build"));

    assert!(project.join("Cargo.toml").exists());
    assert!(project.join("src/lib.rs").exists());
    assert!(project.join(".sdkt.toml").exists());
    assert!(project.join("README.md").exists());
    assert!(project.join(".gitignore").exists());
    assert!(project.join("tests/basic.rs").exists());

    let lib = fs::read_to_string(project.join("src/lib.rs")).unwrap();
    assert!(lib.contains("#![no_std]"));
}

#[test]
fn init_minimal_omits_extras() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("minimal_proj");

    sdkt()
        .args(["init", project.to_str().unwrap(), "--minimal"])
        .assert()
        .success();

    assert!(project.join("Cargo.toml").exists());
    assert!(project.join("src/lib.rs").exists());
    assert!(project.join(".sdkt.toml").exists());
    assert!(!project.join("README.md").exists());
    assert!(!project.join("tests").exists());
}

#[test]
fn init_rejects_existing_directory() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("existing");
    fs::create_dir_all(&project).unwrap();

    sdkt()
        .args(["init", project.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn init_force_overwrites() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("force_proj");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("user_file.txt"), "keep me").unwrap();

    sdkt()
        .args(["init", project.to_str().unwrap(), "--force"])
        .assert()
        .success();

    assert!(project.join("Cargo.toml").exists());
    assert!(project.join("user_file.txt").exists());
}

#[test]
fn init_json_output() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("json_proj");

    sdkt()
        .args(["init", project.to_str().unwrap(), "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\":\"created\""));
}

#[test]
fn init_generated_cargo_toml_valid() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("cargo_check_proj");

    sdkt()
        .args(["init", project.to_str().unwrap(), "--minimal"])
        .assert()
        .success();

    let cargo = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("soroban-sdk"));
    assert!(cargo.contains("[lib]"));
    assert!(cargo.contains("cdylib"));
}

#[test]
fn init_generated_project_compiles_successfully() {
    // This is an integration test protecting M31 (Regression Protection).
    // It verifies that the generated `Cargo.toml` dependency graph
    // (specifically `soroban-sdk`) actually resolves and compiles cleanly
    // without triggering downstream transitive failures (e.g. ethnum transmute aborts).

    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("build_proj");

    sdkt()
        .args(["init", project.to_str().unwrap(), "--minimal"])
        .assert()
        .success();

    // Verify it builds using standard cargo check. We check rather than build
    // to keep the test suite execution time low, but `check` is sufficient
    // to catch trait constraint or transmute failures in the dependency graph.

    let status = StdCommand::new("cargo")
        .arg("check")
        .current_dir(&project)
        .status()
        .expect("cargo check failed to execute");
    assert!(status.success(), "Scaffolded project failed to compile");
}

#[test]
fn init_sdkt_toml_has_network() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("config_proj");

    sdkt()
        .args(["init", project.to_str().unwrap(), "--minimal"])
        .assert()
        .success();

    let sdkt = fs::read_to_string(project.join(".sdkt.toml")).unwrap();
    assert!(sdkt.contains("[network]"));
    assert!(sdkt.contains("testnet"));
}
