use assert_cmd::Command;
use std::io::Write;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt-cli").unwrap()
}

/// Write `content` to `<dir>/contract.rs` and return the path.
fn write_fixture(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    p
}

#[test]
fn audit_help_documents_gap_c() {
    sdkt()
        .args(["audit", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Static security analysis"))
        .stdout(predicates::str::contains("RULE_ID"));
}

#[test]
fn audit_missing_file_errors() {
    sdkt()
        .args(["audit", "/no/such/contract.rs"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("Failed to read source"));
}

#[test]
fn audit_invalid_rust_source_errors() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "bad_syntax.rs", "fn { not rust code ");
    sdkt()
        .args(["audit", path.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("source parse error"));
}

#[test]
fn audit_flags_privileged_without_auth() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "bad.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    sdkt()
        .args(["audit", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("AUTH-001"))
        .stdout(predicates::str::contains("critical"));
}

#[test]
fn audit_clean_source_reports_no_issues() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "ok.rs",
        "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n",
    );
    sdkt()
        .args(["audit", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("No issues found."));
}

#[test]
fn audit_disable_rule_suppresses_finding() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "bad.rs", "pub fn mint_token(to: Address) { }\n");
    sdkt()
        .args(["audit", path.to_str().unwrap(), "--disable", "AUTH-001"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No issues found."));
}

#[test]
fn audit_json_output_is_valid_report() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "bad.rs", "pub fn initialize(admin: Address) { }\n");
    sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"rule_id\":\"AUTH-003\""))
        .stdout(predicates::str::contains("\"findings\""));
}

#[test]
fn audit_rules_flag_accepted_and_default_unchanged() {
    // `--rules` is additive: providing a valid (existing) path must not change
    // the built-in audit output. temp_dir() always exists on the runner.
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "ok.rs",
        "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n",
    );
    sdkt()
        .args([
            "audit",
            path.to_str().unwrap(),
            "--rules",
            std::env::temp_dir().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("No issues found."));
}

#[test]
fn audit_rules_missing_path_errors() {
    sdkt()
        .args([
            "audit",
            "/no/such/contract.rs",
            "--rules",
            "/no/such/rule/dir",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("does not exist"));
}

#[cfg(feature = "plugins")]
#[test]
fn audit_example_plugin_rule_fires_with_plugins_feature() {
    // This test only runs when sdkt-cli is built with `--features plugins`,
    // which links the reference example rule (EXAMPLE-001) into the registry.
    use std::process::Command;
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "trigger.rs",
        "pub fn sdkt_example_trigger(admin: Address) { require_auth(); }\n",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt-cli"))
        .args([
            "audit",
            path.to_str().unwrap(),
            "--rules",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("run sdkt-cli");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("EXAMPLE-001"),
        "example plugin rule should fire"
    );
}
