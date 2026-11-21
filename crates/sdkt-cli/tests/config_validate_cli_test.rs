//! Regression tests for offline `sdkt config validate` behavior.
use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env_remove("HOME");
    cmd
}

#[test]
fn missing_config_uses_defaults_in_json() {
    let dir = TempDir::new().unwrap();
    sdkt()
        .current_dir(dir.path())
        .args(["config", "validate", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""valid":true"#));
}

#[test]
fn invalid_format_is_rejected_before_reading_config() {
    let dir = TempDir::new().unwrap();
    sdkt()
        .current_dir(dir.path())
        .args(["config", "validate", "--format", "jsno"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Invalid format"));
}

#[test]
fn dependency_semantic_error_identifies_git_field() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(".sdkt.toml"),
        "[dependencies.token]\ngit = 'ftp://host/repo'\nbranch = 'main'\n",
    )
    .unwrap();
    let out = sdkt()
        .current_dir(dir.path())
        .args(["config", "validate", "--format", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["problems"][0]["section"], "dependencies.token");
    assert_eq!(json["problems"][0]["key"], "git");
}

#[test]
fn unknown_schema_key_is_reported() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(".sdkt.toml"), "[decode]\nmax_dept = 3\n").unwrap();
    let out = sdkt()
        .current_dir(dir.path())
        .args(["config", "validate", "--format", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["problems"][0]["section"], "decode");
    assert_eq!(json["problems"][0]["key"], "max_dept");
}
