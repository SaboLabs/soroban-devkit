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

#[test]
fn baseline_and_save_baseline_same_file_compares_before_overwriting() {
    let dir = TempDir::new().unwrap();
    let contract_path = write_fixture(
        &dir,
        "token.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let baseline_path = dir.path().join("audit-baseline.json");

    // Save initial baseline (1 finding: mint_token)
    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Add a second finding to token.rs
    std::fs::write(
        &contract_path,
        "pub fn mint_token(to: Address) { /* no auth */ }\n\
         pub fn burn_token(to: Address) { /* no auth */ }\n",
    )
    .unwrap();

    // Run with --baseline and --save-baseline pointing to the SAME file.
    // Must fail because comparison happens against original baseline (1 finding),
    // discovering 1 new finding (burn_token), even though save-baseline updates the file.
    let assert = sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--baseline",
            baseline_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Baseline: 1 known finding"));
    assert!(stdout.contains("New findings: 1"));

    // Verify the baseline was updated on disk to record 2 findings
    let content = std::fs::read_to_string(&baseline_path).unwrap();
    let val: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(val["summary"]["total"], 2);
}

#[test]
fn baseline_includes_file_in_finding_identity_for_multi_file_audits() {
    let dir = TempDir::new().unwrap();
    let a_path = write_fixture(
        &dir,
        "contract_a.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let b_path = write_fixture(&dir, "contract_b.rs", "pub fn safe_token() { }\n");
    let baseline_path = dir.path().join("audit-baseline.json");

    // Run multi-file audit on both files and save baseline
    sdkt()
        .args([
            "audit",
            a_path.to_str().unwrap(),
            b_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Now fix contract_a and introduce the exact same finding into contract_b
    std::fs::write(&a_path, "pub fn safe_token() { }\n").unwrap();
    std::fs::write(
        &b_path,
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    )
    .unwrap();

    // Run multi-file audit against baseline.
    // Even though rule_id ("AUTH-001"), location ("mint_token"), and message are identical,
    // they are in different files (contract_b vs contract_a).
    // Ratchet MUST fail because contract_b's finding is new!
    let assert = sdkt()
        .args([
            "audit",
            a_path.to_str().unwrap(),
            b_path.to_str().unwrap(),
            "--baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("New findings: 1"));
    assert!(stdout.contains("Resolved: 1"));
}

#[test]
fn save_baseline_excludes_disabled_rules_from_metadata() {
    let dir = TempDir::new().unwrap();
    let contract_path = write_fixture(
        &dir,
        "token.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let baseline_path = dir.path().join("audit-baseline.json");

    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--disable",
            "AUTH-001",
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(&baseline_path).unwrap();
    let val: serde_json::Value = serde_json::from_str(&content).unwrap();
    let rules: Vec<String> = val["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect();

    assert!(!rules.contains(&"AUTH-001".to_string()));
    assert!(rules.contains(&"AUTH-002".to_string()));
}

#[test]
fn save_baseline_does_not_overwrite_after_partial_audit_failure() {
    let dir = TempDir::new().unwrap();
    let valid_path = write_fixture(
        &dir,
        "valid.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let invalid_path = dir.path().join("nonexistent.rs");
    let baseline_path = dir.path().join("audit-baseline.json");

    // Save initial good baseline
    sdkt()
        .args([
            "audit",
            valid_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let original_content = std::fs::read_to_string(&baseline_path).unwrap();

    // Run multi-file audit with one valid file and one nonexistent file.
    // Must exit with error and must NOT overwrite the baseline.
    sdkt()
        .args([
            "audit",
            valid_path.to_str().unwrap(),
            invalid_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let after_content = std::fs::read_to_string(&baseline_path).unwrap();
    assert_eq!(original_content, after_content);
}

#[test]
fn baseline_with_fail_on_only_fails_for_new_findings_meeting_threshold() {
    let dir = TempDir::new().unwrap();
    // Start with 1 critical finding (AUTH-001)
    let contract_path = write_fixture(
        &dir,
        "token.rs",
        "pub fn mint_token(to: Address) { /* no auth */ }\n",
    );
    let baseline_path = dir.path().join("audit-baseline.json");

    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--save-baseline",
            baseline_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Introduce a warning finding (MATH-001)
    std::fs::write(
        &contract_path,
        "pub fn mint_token(to: Address) { /* no auth */ }\n\
         pub fn calc(a: u32, b: u32, c: u32) -> u32 {\n\
             (a / b) * c\n\
         }\n",
    )
    .unwrap();

    // With --baseline and --fail-on critical:
    // Existing AUTH-001 is critical, but it is known (in baseline).
    // The NEW finding is MATH-001 (warning).
    // Since no NEW findings are >= critical, this must exit 0!
    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--baseline",
            baseline_path.to_str().unwrap(),
            "--fail-on",
            "critical",
        ])
        .assert()
        .success();

    // With --baseline and --fail-on warning:
    // The new MATH-001 is warning >= warning, so this must exit 1!
    sdkt()
        .args([
            "audit",
            contract_path.to_str().unwrap(),
            "--baseline",
            baseline_path.to_str().unwrap(),
            "--fail-on",
            "warning",
        ])
        .assert()
        .failure();
}

#[test]
fn baseline_with_sarif_is_rejected() {
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
            "--format",
            "sarif",
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("--format sarif is not supported with --baseline or --save-baseline"));
}
