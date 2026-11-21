use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env_remove("HOME");
    cmd
}

fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(".sdkt.toml"),
        r#"
            [network]
            rpc_url = "https://example.test/rpc"
            passphrase = "Example Network"
            timeout_secs = 15
            pool_max_idle_per_host = 100

            [decode]
            max_depth = 42
        "#,
    )
    .unwrap();
    dir
}

#[test]
fn config_show_json_contains_resolved_fixture_values_without_secrets() {
    let dir = fixture();
    let output = sdkt()
        .current_dir(dir.path())
        .args(["config", "show", "--format", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["network"]["rpc_url"], "https://example.test/rpc");
    assert_eq!(json["decode"]["max_depth"], 42);
    let text = serde_json::to_string(&json).unwrap().to_lowercase();
    assert!(!text.contains("private"));
    assert!(!text.contains("secret"));
}

#[test]
fn config_show_without_file_prints_defaults_successfully() {
    let dir = TempDir::new().unwrap();
    sdkt()
        .current_dir(dir.path())
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No .sdkt.toml found"))
        .stdout(predicates::str::contains("soroban-testnet.stellar.org"));
}

#[test]
fn config_show_keeps_empty_configuration_as_an_object() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(".sdkt.toml"), "").unwrap();

    let json_output = sdkt()
        .current_dir(dir.path())
        .args(["config", "show", "--format", "json"])
        .output()
        .unwrap();
    assert!(json_output.status.success());
    let json: Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(json, serde_json::json!({}));

    let pretty_output = sdkt()
        .current_dir(dir.path())
        .args(["config", "show"])
        .output()
        .unwrap();
    assert!(pretty_output.status.success());
    assert!(String::from_utf8_lossy(&pretty_output.stdout)
        .contains("value(s) are at built-in defaults"));
}

#[test]
fn config_show_redacts_rpc_url_password_in_json_and_pretty_with_defaults() {
    let dir = fixture();
    let config_path = dir.path().join(".sdkt.toml");
    let config = fs::read_to_string(&config_path).unwrap().replace(
        "https://example.test/rpc",
        "https://rpc-user:dummy-password@example.test/rpc",
    );
    fs::write(&config_path, config).unwrap();

    for args in [
        vec!["config", "show", "--format", "json", "--show-defaults"],
        vec!["config", "show", "--show-defaults"],
    ] {
        let output = sdkt().current_dir(dir.path()).args(args).output().unwrap();
        assert!(output.status.success());
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("rpc-user:***@example.test/rpc"));
        assert!(!text.contains("dummy-password"));
    }
}
