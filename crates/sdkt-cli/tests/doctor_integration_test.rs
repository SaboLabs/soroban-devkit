//! Integration tests for `sdkt doctor`.
//!
//! Environment-independence contract: these tests MUST pass on any host
//! (developer machine, ubuntu/macos/windows CI runners) regardless of which
//! rustup targets are installed. The host-dependent `wasm-target`/`rust-toolchain`
//! checks are therefore asserted through the doctor CONTRACT (exit code is 0
//! exactly when no check has status "error"; project-config reflects the CWD
//! which the test controls) — never through an assumed toolchain state.
//! The deterministic logic of those checks is covered by unit tests over the
//! pure functions in main.rs (`wasm_target_present`, `path_contains_command`).

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

/// Run `sdkt doctor --json` in `dir`, returning (exit_code, parsed_report).
fn doctor_json_in(dir: &TempDir) -> (Option<i32>, serde_json::Value) {
    let output = sdkt()
        .args(["doctor", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("doctor runs");
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("doctor --json must emit valid JSON");
    (output.status.code(), json)
}

/// True when any check has status "error".
fn has_error(json: &serde_json::Value) -> bool {
    json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["status"] == "error")
}

#[test]
fn doctor_pretty_reports_all_core_checks() {
    // Pretty output is environment-independent for its STRUCTURE: header,
    // check ids, and the Result verdict line must always appear.
    let tmp = TempDir::new().unwrap();
    let output = sdkt()
        .arg("doctor")
        .current_dir(tmp.path())
        .output()
        .expect("doctor runs");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sdkt doctor"));
    assert!(stdout.contains("sdkt-runtime"));
    assert!(stdout.contains("rust-toolchain"));
    assert!(stdout.contains("wasm-target"));
    assert!(stdout.contains("project-config"));
    assert!(
        stdout.contains("Result: HEALTHY") || stdout.contains("Result: UNHEALTHY"),
        "a verdict line must be printed"
    );
}

#[test]
fn doctor_clean_dir_is_healthy_exactly_when_no_check_errors() {
    // The doctor CONTRACT: exit 0 ⇔ healthy ⇔ no error status. A clean temp
    // dir has no .sdkt.toml, so project-config must be a *warning* (never an
    // error) — the only thing that can make it unhealthy is a genuinely
    // missing toolchain on the host, which doctor must then report honestly.
    let tmp = TempDir::new().unwrap();
    let (code, json) = doctor_json_in(&tmp);
    assert!(json["healthy"].is_boolean());
    assert_eq!(
        json["healthy"].as_bool().unwrap(),
        !has_error(&json),
        "healthy must equal the absence of error checks"
    );
    assert_eq!(
        code == Some(0),
        json["healthy"].as_bool().unwrap(),
        "exit code 0 must equal healthy"
    );

    // The project-config part is host-independent: no .sdkt.toml in a fresh
    // TempDir, so it must be a warning with a remediation hint.
    let project = json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "project-config")
        .expect("project-config check present");
    assert_eq!(project["status"], "warning");
    assert!(project.get("remediation").is_some());
}

#[test]
fn doctor_json_structure_has_checks_array() {
    let tmp = TempDir::new().unwrap();
    let (_code, json) = doctor_json_in(&tmp);
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
    // Runs on any host regardless of health verdict — the secret-leak
    // property holds for the report content itself.
    let tmp = TempDir::new().unwrap();
    let (_code, json) = doctor_json_in(&tmp);
    let stdout = serde_json::to_string(&json).unwrap();
    for forbidden in ["secret", "private_key", "seed", "password", "S-"] {
        assert!(
            !stdout.contains(forbidden),
            "doctor must not mention {forbidden}"
        );
    }
}

#[test]
fn doctor_exit_code_nonzero_on_invalid_config() {
    // An unparseable .sdkt.toml is an ERROR regardless of host toolchain, so
    // doctor must exit 1 / report UNHEALTHY deterministically everywhere.
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
    assert!(stdout.contains("project-config"));
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
fn doctor_json_reports_error_status_on_invalid_config() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".sdkt.toml"), "[network]\n").unwrap();
    let (code, json) = doctor_json_in(&tmp);
    assert_eq!(code, Some(1));
    assert_eq!(
        json["healthy"], false,
        "healthy must be false on invalid config"
    );
    let project_check = json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "project-config")
        .unwrap();
    assert_eq!(project_check["status"], "error");
    assert!(has_error(&json));
}

#[test]
fn doctor_valid_project_config_is_ok() {
    // A parseable .sdkt.toml with at least one contract and a consistent
    // dependency graph must produce project-config = ok on any host —
    // validate_project checks the graph, not artifact existence (that is
    // `sdkt build`'s job), so this is CWD-controlled and host-independent.
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join(".sdkt.toml"),
        "[network]\nrpc_url = \"https://soroban-testnet.stellar.org\"\npassphrase = \"Test SDF Network ; September 2015\"\n\n[contracts.token]\npath = \"contracts/token\"\n",
    )
    .unwrap();
    let (_code, json) = doctor_json_in(&tmp);
    let project_check = json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "project-config")
        .unwrap();
    assert_eq!(project_check["status"], "ok");
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
