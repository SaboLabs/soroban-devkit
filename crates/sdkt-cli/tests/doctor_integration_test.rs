use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

#[test]
fn doctor_pretty_succeeds_in_non_project_dir() {
    let tmp = TempDir::new().unwrap();
    let output = sdkt()
        .arg("doctor")
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    assert!(output.status.success(), "doctor should exit 0");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sdkt doctor"));
    assert!(stdout.contains("sdkt-runtime"));
    assert!(stdout.contains("project-config"));
    assert!(stdout.contains("HEALTHY"));
}

#[test]
fn doctor_json_succeeds_in_non_project_dir() {
    let tmp = TempDir::new().unwrap();
    let output = sdkt()
        .arg("doctor")
        .arg("--json")
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"healthy\": true"));
    assert!(stdout.contains("\"id\": \"sdkt-runtime\""));
    assert!(stdout.contains("\"status\": \"ok\""));
}

#[test]
fn doctor_json_structure_has_checks_array() {
    let tmp = TempDir::new().unwrap();
    let output = sdkt()
        .args(["doctor", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("doctor --json must emit valid JSON");
    assert!(json["checks"].is_array(), "checks must be an array");
    assert!(json["healthy"].is_boolean(), "healthy must be a boolean");
    let checks = json["checks"].as_array().unwrap();
    assert!(!checks.is_empty(), "checks must not be empty");
    for check in checks {
        assert!(check["id"].is_string(), "each check needs an id");
        assert!(check["status"].is_string(), "each check needs a status");
        assert!(check["message"].is_string(), "each check needs a message");
    }
}

#[test]
fn doctor_does_not_leak_secrets() {
    let tmp = TempDir::new().unwrap();
    let output = sdkt()
        .args(["doctor", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("secret"),
        "doctor must not mention secrets"
    );
    assert!(
        !stdout.contains("private_key"),
        "doctor must not mention private keys"
    );
    assert!(!stdout.contains("seed"), "doctor must not mention seeds");
    assert!(
        !stdout.contains("password"),
        "doctor must not mention passwords"
    );
}

#[test]
fn doctor_exit_code_zero_when_healthy() {
    let tmp = TempDir::new().unwrap();
    let output = sdkt()
        .arg("doctor")
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn doctor_exit_code_nonzero_on_invalid_config() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".sdkt.toml"), "[network]\n").unwrap();
    let output = sdkt()
        .arg("doctor")
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("UNHEALTHY"));
}

#[test]
fn doctor_exit_code_nonzero_on_invalid_dependency_graph() {
    let tmp = TempDir::new().unwrap();
    let config = r#"
[network]
rpc_url = "https://soroban-testnet.stellar.org"
passphrase = "Test SDF Network ; September 2015"

[contracts.token]
path = "contracts/token"

[contracts.router]
path = "contracts/router"
depends_on = ["ghost"]
"#;
    fs::write(tmp.path().join(".sdkt.toml"), config).unwrap();
    let output = sdkt()
        .arg("doctor")
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("project-config"));
    assert!(stdout.contains("UNHEALTHY"));
}

#[test]
fn doctor_json_exit_code_nonzero_on_invalid_config() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".sdkt.toml"), "[network]\n").unwrap();
    let output = sdkt()
        .args(["doctor", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(
        json["healthy"], false,
        "healthy must be false on invalid config"
    );
    let checks = json["checks"].as_array().unwrap();
    let project_check = checks.iter().find(|c| c["id"] == "project-config").unwrap();
    assert_eq!(project_check["status"], "error");
}

#[test]
fn doctor_pretty_shows_remediation_on_error() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".sdkt.toml"), "[network]\n").unwrap();
    let output = sdkt()
        .arg("doctor")
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("fix:"));
}

#[test]
fn doctor_help_text() {
    let output = sdkt()
        .arg("doctor")
        .arg("--help")
        .output()
        .expect("doctor --help runs");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("diagnostic"));
    assert!(stdout.contains("--json"));
}
