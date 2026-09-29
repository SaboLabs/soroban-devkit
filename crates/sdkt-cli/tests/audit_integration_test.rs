use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
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
        .stderr(predicates::str::contains("Failed to parse Rust source"));
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
fn audit_division_before_multiplication_reports_json_and_can_be_disabled() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "arithmetic.rs",
        "pub fn quote(amount: i128, bps: i128) -> i128 { amount / 10_000 * bps }\n",
    );
    let output = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "json"])
        .output()
        .expect("run audit JSON");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let finding = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["rule_id"] == "MATH-001")
        .expect("MATH-001 finding should be emitted");
    assert_eq!(finding["severity"], "warning");
    assert_eq!(finding["location"], "quote");

    sdkt()
        .args(["audit", path.to_str().unwrap(), "--disable", "MATH-001"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No issues found."));
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

#[test]
fn audit_flags_transfer_without_auth() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "transfer_no_auth.rs",
        "pub fn transfer(from: Address, to: Address, amount: i128) { }\n",
    );
    sdkt()
        .args(["audit", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("AUTH-004"))
        .stdout(predicates::str::contains("critical"));
}

#[test]
fn audit_transfer_with_auth_not_flagged() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "transfer_auth.rs",
        "pub fn transfer(from: Address, to: Address, amount: i128) { require_auth(); }\n",
    );
    sdkt()
        .args(["audit", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("AUTH-004").not());
}

#[test]
fn audit_disable_auth004_suppresses_finding() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "transfer_no_auth.rs",
        "pub fn transfer(from: Address, to: Address, amount: i128) { }\n",
    );
    sdkt()
        .args(["audit", path.to_str().unwrap(), "--disable", "AUTH-004"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No issues found."));
}

#[cfg(feature = "plugins")]
#[test]
fn audit_example_plugin_rule_fires_with_plugins_feature() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "trigger.rs",
        "pub fn sdkt_example_trigger(admin: Address) { require_auth(); }\n",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_sdkt"))
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

#[test]
fn audit_directory_walks_nested_rust_files() {
    let dir = TempDir::new().unwrap();
    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).unwrap();

    write_fixture(&dir, "bad.rs", "pub fn mint_token(to: Address) { }\n");

    std::fs::write(
        nested.join("transfer.rs"),
        "pub fn transfer(from: Address, to: Address, amount: i128) { }\n",
    )
    .unwrap();

    sdkt()
        .args(["audit", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("bad.rs"))
        .stdout(predicates::str::contains("transfer.rs"))
        .stdout(predicates::str::contains("Aggregate Severity"));
}

#[test]
fn audit_directory_json_contains_per_file_reports_and_summary() {
    let dir = TempDir::new().unwrap();

    write_fixture(&dir, "a.rs", "pub fn mint_token(to: Address) { }\n");
    write_fixture(
        &dir,
        "b.rs",
        "pub fn transfer(from: Address, to: Address, amount: i128) { }\n",
    );

    sdkt()
        .args(["audit", dir.path().to_str().unwrap(), "--format", "json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"files\""))
        .stdout(predicates::str::contains("\"summary\""))
        .stdout(predicates::str::contains("\"file\""))
        .stdout(predicates::str::contains("a.rs"))
        .stdout(predicates::str::contains("b.rs"));
}

#[test]
fn audit_multiple_explicit_files_json_contains_both_files_and_summary() {
    let dir = TempDir::new().unwrap();
    let a = write_fixture(&dir, "a.rs", "pub fn mint_token(to: Address) { }\n");
    let b = write_fixture(
        &dir,
        "b.rs",
        "pub fn transfer(from: Address, to: Address, amount: i128) { }\n",
    );

    let out = sdkt()
        .args([
            "audit",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON output");
    let files = v["files"].as_array().expect("files array");
    let summary = &v["summary"];

    let a_file = files
        .iter()
        .find(|entry| {
            entry["file"]
                .as_str()
                .and_then(|path| std::path::Path::new(path).file_name())
                .and_then(|name| name.to_str())
                == Some("a.rs")
        })
        .expect("a.rs entry in JSON output");
    let b_file = files
        .iter()
        .find(|entry| {
            entry["file"]
                .as_str()
                .and_then(|path| std::path::Path::new(path).file_name())
                .and_then(|name| name.to_str())
                == Some("b.rs")
        })
        .expect("b.rs entry in JSON output");

    assert!(
        a_file["report"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["rule_id"] == "AUTH-001"),
        "a.rs should contain AUTH-001"
    );
    assert!(
        b_file["report"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["rule_id"] == "AUTH-004"),
        "b.rs should contain AUTH-004"
    );

    let per_file_total: usize = files
        .iter()
        .map(|entry| entry["report"]["summary"]["total"].as_u64().unwrap() as usize)
        .sum();
    let aggregate_total = summary["total"].as_u64().unwrap() as usize;

    assert_eq!(
        per_file_total, aggregate_total,
        "summary totals should reconcile"
    );
}

#[test]
fn audit_directory_continues_after_unparseable_file() {
    let dir = TempDir::new().unwrap();

    write_fixture(&dir, "bad_syntax.rs", "fn { not rust code ");
    write_fixture(&dir, "valid.rs", "pub fn mint_token(to: Address) { }\n");

    sdkt()
        .args(["audit", dir.path().to_str().unwrap(), "--format", "json"])
        .assert()
        .failure()
        .stdout(predicates::str::contains("bad_syntax.rs"))
        .stdout(predicates::str::contains("valid.rs"))
        .stdout(predicates::str::contains("AUDIT-PARSE"));
}

#[test]
fn audit_list_rules_pretty_matches_registry() {
    let all = sdkt_audit::all_rules();
    let expected_header = format!("Available audit rules ({}):", all.len());
    let mut assert = sdkt().args(["audit", "--list-rules"]).assert().success();
    assert = assert.stdout(predicates::str::contains(&expected_header));
    for rule in &all {
        assert = assert.stdout(predicates::str::contains(rule.id()));
        assert = assert.stdout(predicates::str::contains(rule.severity().to_string()));
        assert = assert.stdout(predicates::str::contains(rule.description()));
    }
}

#[test]
fn audit_list_rules_json_shape_and_content() {
    let all = sdkt_audit::all_rules();
    let out = sdkt()
        .args(["audit", "--list-rules", "--format", "json"])
        .output()
        .expect("run sdkt audit --list-rules --format json");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");

    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let arr = v.as_array().expect("JSON array");
    assert_eq!(arr.len(), all.len());

    for (elem, rule) in arr.iter().zip(all.iter()) {
        let obj = elem.as_object().expect("element is an object");
        assert_eq!(obj.len(), 3);
        assert_eq!(obj["id"].as_str().unwrap(), rule.id());
        assert_eq!(
            obj["severity"].as_str().unwrap(),
            rule.severity().to_string()
        );
        assert_eq!(obj["description"].as_str().unwrap(), rule.description());
    }

    let rule_infos: Vec<sdkt_audit::RuleInfo> =
        serde_json::from_str(&stdout).expect("deserializable into RuleInfo");
    assert_eq!(rule_infos.len(), all.len());
    for (info, rule) in rule_infos.iter().zip(all.iter()) {
        assert_eq!(info.id, rule.id());
        assert_eq!(info.severity, rule.severity());
        assert_eq!(info.description, rule.description());
    }
}

#[test]
fn audit_list_rules_deterministic_across_runs() {
    let out1 = sdkt()
        .args(["audit", "--list-rules"])
        .output()
        .expect("run 1");
    let out2 = sdkt()
        .args(["audit", "--list-rules"])
        .output()
        .expect("run 2");
    let out3 = sdkt()
        .args(["audit", "--list-rules"])
        .output()
        .expect("run 3");
    assert_eq!(out1.stdout, out2.stdout);
    assert_eq!(out2.stdout, out3.stdout);

    let json1 = sdkt()
        .args(["audit", "--list-rules", "--format", "json"])
        .output()
        .expect("json run 1");
    let json2 = sdkt()
        .args(["audit", "--list-rules", "--format", "json"])
        .output()
        .expect("json run 2");
    assert_eq!(json1.stdout, json2.stdout);
}

#[test]
fn audit_list_rules_disable_flag_unaffected() {
    let base_out = sdkt()
        .args(["audit", "--list-rules"])
        .output()
        .expect("run base");
    let disable_out = sdkt()
        .args(["audit", "--list-rules", "--disable", "AUTH-001"])
        .output()
        .expect("run with disable");
    assert_eq!(base_out.stdout, disable_out.stdout);

    let base_json = sdkt()
        .args(["audit", "--list-rules", "--format", "json"])
        .output()
        .expect("run base json");
    let disable_json = sdkt()
        .args([
            "audit",
            "--list-rules",
            "--disable",
            "AUTH-001",
            "--format",
            "json",
        ])
        .output()
        .expect("run disable json");
    assert_eq!(base_json.stdout, disable_json.stdout);
}

#[test]
fn audit_list_rules_without_path_succeeds() {
    sdkt()
        .args(["audit", "--list-rules"])
        .assert()
        .success()
        .stdout(predicates::str::contains("AUTH-001"));
}

#[test]
fn audit_list_rules_ignores_dummy_path() {
    sdkt()
        .args(["audit", "--list-rules", "/path/that/does/not/exist.rs"])
        .assert()
        .success()
        .stdout(predicates::str::contains("AUTH-001"));
}

#[test]
fn audit_missing_path_fails_when_list_rules_not_specified() {
    sdkt()
        .args(["audit"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("PATH"));
}
