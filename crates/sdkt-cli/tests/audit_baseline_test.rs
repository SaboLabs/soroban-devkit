use assert_cmd::Command;
use std::io::Write;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

fn write_fixture(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    p
}

#[test]
fn save_baseline_persists_valid_json_and_prints_saved_message() {
    let dir = TempDir::new().unwrap();
    let contract_path = write_fixture(
        &dir,
        "token.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let baseline_path = dir.path().join("audit-baseline.json");

    let assert = sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Report saved (1 finding:"));

    // Verify baseline file exists and contains valid JSON metadata + findings
    assert!(baseline_path.exists());
    let content = std::fs::read_to_string(&baseline_path).unwrap();
    let val: serde_json::Value = serde_json::from_str(&content).unwrap();

    assert!(val.get("version").is_some());
    assert!(val["rules"].as_array().is_some_and(|r| !r.is_empty()));
    assert_eq!(val["summary"]["total"], 1);
    assert_eq!(val["findings"].as_array().unwrap().len(), 1);
    assert_eq!(val["findings"][0]["rule_id"], "AUTH-001");
}

#[test]
fn baseline_ratchet_identical_findings_exits_zero() {
    let dir = TempDir::new().unwrap();
    let contract_path = write_fixture(
        &dir,
        "token.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let baseline_path = dir.path().join("audit-baseline.json");

    // Save baseline
    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Re-run against identical code with --baseline
    let assert = sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Baseline: 1 known finding"));
    assert!(stdout.contains("New findings: 0"));
    assert!(stdout.contains("Resolved: 0"));
}

#[test]
fn baseline_ratchet_new_finding_exits_non_zero_and_names_violation() {
    let dir = TempDir::new().unwrap();
    let contract_path = write_fixture(
        &dir,
        "token.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let baseline_path = dir.path().join("audit-baseline.json");

    // Save initial baseline (1 finding: AUTH-001 mint_token)
    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Introduce a second finding: burn_token
    std::fs::write(
        &contract_path,
        "pub fn mint_token(to: Address) { /* no auth */ }\n\
         pub fn burn_token(to: Address) { /* no auth */ }\n",
    )
    .unwrap();

    // Audit against baseline must fail (exit 1) and name the new finding
    let assert = sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Baseline: 1 known finding"));
    assert!(stdout.contains("New findings: 1"));
    assert!(stdout.contains("AUTH-001"));
    assert!(stdout.contains("burn_token"));
    assert!(stdout.contains("Resolved: 0"));
}

#[test]
fn baseline_ratchet_subset_exits_zero_and_reports_resolved() {
    let dir = TempDir::new().unwrap();
    let contract_path = write_fixture(
        &dir,
        "token.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n\
         pub fn burn_token(to: Address) { /* no auth */ }\n",
    );
    let baseline_path = dir.path().join("audit-baseline.json");

    // Save baseline with 2 findings
    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Resolve one finding (remove burn_token)
    std::fs::write(
        &contract_path,
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    )
    .unwrap();

    // Audit against baseline passes (exit 0) and reports 1 resolved
    let assert = sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Baseline: 2 known findings"));
    assert!(stdout.contains("New findings: 0"));
    assert!(stdout.contains("Resolved: 1"));
}

#[test]
fn baseline_json_format_emits_structured_diff() {
    let dir = TempDir::new().unwrap();
    let contract_path = write_fixture(
        &dir,
        "token.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let baseline_path = dir.path().join("audit-baseline.json");

    // Save baseline
    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Check with --format json
    let assert = sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--baseline",
            baseline_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(val["baseline_count"], 1);
    assert_eq!(val["known_count"], 1);
    assert_eq!(val["new_count"], 0);
    assert_eq!(val["resolved_count"], 0);
    assert_eq!(val["pass"], true);
}

#[test]
fn stale_rules_baseline_triggers_warning_on_stderr() {
    let dir = TempDir::new().unwrap();
    let contract_path = write_fixture(
        &dir,
        "token.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let baseline_path = dir.path().join("audit-baseline.json");

    // First save real baseline
    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Tamper the rules list in the saved baseline to simulate stale rule set
    let content = std::fs::read_to_string(&baseline_path).unwrap();
    let mut val: serde_json::Value = serde_json::from_str(&content).unwrap();
    val["rules"] = serde_json::json!(["DUMMY-RULE-ONLY"]);
    std::fs::write(&baseline_path, serde_json::to_string_pretty(&val).unwrap()).unwrap();

    let assert = sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("Warning:"));
    assert!(stderr.contains("different rule set"));
}
