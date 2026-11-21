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
