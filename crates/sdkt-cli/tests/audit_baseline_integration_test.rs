//! Integration tests for `sdkt audit --save-baseline` / `--baseline`.
//!
//! Covers the ratchet workflow end-to-end: persist a baseline, then gate a
//! later run on *new* findings only.

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

/// Write `content` to `<dir>/<name>` and return the path.
fn write_fixture(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    p
}

/// A source that triggers AUTH-001 (privileged fn without auth).
const BAD_SRC: &str = "pub fn mint_token(to: Address) { /* no auth */ }\n";
/// A source with no findings.
const CLEAN_SRC: &str = "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n";

#[test]
fn audit_save_baseline_persists_structured_report() {
    let dir = TempDir::new().unwrap();
    let src = write_fixture(&dir, "bad.rs", BAD_SRC);
    let baseline = dir.path().join("audit-baseline.json");

    sdkt()
        .args([
            "audit",
            src.to_str().unwrap(),
            "--save-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(baseline.exists(), "baseline file should be written");

    let saved = sdkt_audit::AuditBaseline::load(&baseline).expect("valid baseline JSON");
    assert_eq!(saved.report.summary.total, 1);
    assert!(saved
        .report
        .findings
        .iter()
        .any(|f| f.rule_id == "AUTH-001"));
    assert_eq!(saved.format_version, sdkt_audit::BASELINE_FORMAT_VERSION);
    assert!(!saved.sdkt_version.is_empty());
    assert!(saved.rules.iter().any(|r| r == "AUTH-001"));
}

#[test]
fn audit_baseline_identical_findings_exit_zero() {
    let dir = TempDir::new().unwrap();
    let src = write_fixture(&dir, "bad.rs", BAD_SRC);
    let baseline = dir.path().join("baseline.json");

    sdkt()
        .args([
            "audit",
            src.to_str().unwrap(),
            "--save-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();

    sdkt()
        .args([
            "audit",
            src.to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("New findings: 0"))
        .stdout(predicates::str::contains("Resolved: 0"));
}

#[test]
fn audit_baseline_new_finding_exits_nonzero_and_names_it() {
    let dir = TempDir::new().unwrap();
    let clean = write_fixture(&dir, "clean.rs", CLEAN_SRC);
    let baseline = dir.path().join("baseline.json");

    // Baseline captured from a clean contract (zero known findings).
    sdkt()
        .args([
            "audit",
            clean.to_str().unwrap(),
            "--save-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Introduce a violation and re-audit against the baseline.
    let bad = write_fixture(&dir, "bad.rs", BAD_SRC);
    sdkt()
        .args([
            "audit",
            bad.to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicates::str::contains("New findings: 1"))
        .stdout(predicates::str::contains("AUTH-001"));
}

#[test]
fn audit_baseline_reports_resolved_findings() {
    let dir = TempDir::new().unwrap();
    let bad = write_fixture(&dir, "bad.rs", BAD_SRC);
    let baseline = dir.path().join("baseline.json");

    sdkt()
        .args([
            "audit",
            bad.to_str().unwrap(),
            "--save-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();

    let clean = write_fixture(&dir, "clean.rs", CLEAN_SRC);
    sdkt()
        .args([
            "audit",
            clean.to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("New findings: 0"))
        .stdout(predicates::str::contains("Resolved: 1"));
}

#[test]
fn audit_baseline_json_output_is_valid_comparison() {
    let dir = TempDir::new().unwrap();
    let bad = write_fixture(&dir, "bad.rs", BAD_SRC);
    let baseline = dir.path().join("baseline.json");

    sdkt()
        .args([
            "audit",
            bad.to_str().unwrap(),
            "--save-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();

    let out = sdkt()
        .args([
            "audit",
            bad.to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("run audit --baseline --format json");
    assert!(out.status.success());

    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("comparison output is JSON");
    assert_eq!(v["new_count"].as_u64().unwrap(), 0);
    assert!(v["passed"].as_bool().unwrap());
    assert!(v["known_findings"].as_u64().unwrap() >= 1);
    assert!(v["new_findings"].is_array());
    assert!(v["resolved_findings"].is_array());
}

#[test]
fn audit_baseline_stale_rule_set_warns() {
    let dir = TempDir::new().unwrap();
    let bad = write_fixture(&dir, "bad.rs", BAD_SRC);
    let baseline = dir.path().join("baseline.json");

    sdkt()
        .args([
            "audit",
            bad.to_str().unwrap(),
            "--save-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Disabling a rule changes the active rule set relative to the baseline.
    // It only removes rules, so no new findings appear: exit stays zero.
    sdkt()
        .args([
            "audit",
            bad.to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
            "--disable",
            "MOVE-001",
        ])
        .assert()
        .success()
        .stderr(predicates::str::contains("Warning:"))
        .stderr(predicates::str::contains("rule set differs"));
}

#[test]
fn audit_baseline_stale_version_warns() {
    let dir = TempDir::new().unwrap();
    let bad = write_fixture(&dir, "bad.rs", BAD_SRC);
    let baseline = dir.path().join("baseline.json");

    sdkt()
        .args([
            "audit",
            bad.to_str().unwrap(),
            "--save-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();

    let mut saved = sdkt_audit::AuditBaseline::load(&baseline).unwrap();
    saved.sdkt_version = "0.0.0-old".to_string();
    saved.save(&baseline).unwrap();

    sdkt()
        .args([
            "audit",
            bad.to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicates::str::contains("Warning:"))
        .stderr(predicates::str::contains("0.0.0-old"));
}

#[test]
fn audit_baseline_missing_file_errors() {
    let dir = TempDir::new().unwrap();
    let src = write_fixture(&dir, "bad.rs", BAD_SRC);

    sdkt()
        .args([
            "audit",
            src.to_str().unwrap(),
            "--baseline",
            dir.path().join("nope.json").to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("Error reading baseline"));
}

#[test]
fn audit_without_baseline_flags_is_unchanged() {
    let dir = TempDir::new().unwrap();
    let src = write_fixture(&dir, "bad.rs", BAD_SRC);

    // No new flags: the classic report and zero exit code are preserved.
    sdkt()
        .args(["audit", src.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("Static Analysis Report"))
        .stdout(predicates::str::contains("AUTH-001"));
}
