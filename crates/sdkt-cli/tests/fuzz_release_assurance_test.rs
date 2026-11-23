//! Release-assurance integration test: a completed fuzz campaign's evidence
//! file attaches to `sdkt release-assurance` and folds into the aggregate.
//!
//! The extension point is the existing `ReleaseAssuranceReport` composition:
//! `--fuzz-evidence <file>` records what an already-completed campaign
//! produced. Release assurance never runs a fuzz campaign itself.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_str()
        .expect("utf-8 fixture path")
        .to_string()
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sdkt-fuzz-ra-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn sdkt(args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("built binary");
    cmd.args(args);
    cmd
}

/// Run a fuzz campaign that produces findings, return its artifact dir.
fn campaign_with_findings() -> PathBuf {
    let out = temp_dir("ra-findings");
    sdkt(&[
        "fuzz",
        "campaign",
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--seed",
        "23",
        "--expect-error",
        "hello:Storage:3",
        "--artifact-dir",
        out.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(1);
    out
}

#[test]
fn release_assurance_records_fuzz_evidence() {
    let out = campaign_with_findings();
    let evidence = out.join("campaign-evidence.json");
    assert!(evidence.exists(), "campaign writes its evidence file");

    // Release assurance with the evidence attached: the fuzz section appears
    // and the aggregate reflects the findings (FAIL).
    let report = temp_dir("ra-report");
    let report_path = report.join("ra.json");
    sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        evidence.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .code(1)
    .stdout(predicate::str::contains("Fuzz Evidence"))
    .stdout(predicate::str::contains("FAIL"));

    // The JSON report carries the evidence fields.
    let text = std::fs::read_to_string(&report_path).unwrap_or_default();
    let _ = text; // the command prints to stdout; parse that instead
    let out = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        evidence.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .code(1);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let value: Value = serde_json::from_str(&stdout).expect("release-assurance JSON");
    let fuzz = value["fuzz"].as_object().expect("fuzz section present");
    assert_eq!(fuzz["status"], "FAIL");
    let ev = &fuzz["evidence"];
    assert_eq!(ev["campaign_completed"], true);
    assert!(ev["findings"].as_array().is_some_and(|f| !f.is_empty()));
    assert_eq!(ev["deterministic"], true);
    assert!(ev["artifacts_available"].as_bool().unwrap_or(false));
    assert!(ev["replay"].as_object().is_some());
}

#[test]
fn release_assurance_without_fuzz_evidence_is_unchanged() {
    // No --fuzz-evidence: the report has no fuzz section at all (the
    // existing release-assurance output is byte-identical to pre-Phase-3).
    let out = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--format",
        "json",
    ])
    .assert()
    .success()
    .code(0);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let value: Value = serde_json::from_str(&stdout).expect("release-assurance JSON");
    assert!(
        value.get("fuzz").is_none(),
        "without --fuzz-evidence the report must not gain a fuzz section"
    );
}

#[test]
fn release_assurance_rejects_malformed_fuzz_evidence() {
    let dir = temp_dir("ra-bad");
    let bad = dir.join("broken.json");
    std::fs::write(&bad, b"not json").expect("write broken evidence");

    sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        bad.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .code(1)
    .stderr(predicate::str::contains("fuzz evidence"));
}

#[test]
fn release_assurance_does_not_pass_incomplete_or_zero_case_fuzz_evidence() {
    let dir = temp_dir("ra-zero-cases");

    // 1. Evidence produced by CampaignEvidence::from_result with executed = 0
    //    has campaign_completed = false and case_count = 0.
    let zero_result = sdkt_fuzz::CampaignResult::default();
    let incomplete_ev = sdkt_fuzz::CampaignEvidence::from_result(
        &zero_result,
        sdkt_fuzz::CampaignConfig::default(),
        &[],
        &std::collections::BTreeMap::new(),
        true,
    );
    assert!(!incomplete_ev.campaign_completed);
    let incomplete_path = dir.join("incomplete-evidence.json");
    std::fs::write(
        &incomplete_path,
        serde_json::to_string_pretty(&incomplete_ev).expect("serialize incomplete evidence"),
    )
    .expect("write incomplete evidence");

    let out = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        incomplete_path.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .code(1);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let value: Value = serde_json::from_str(&stdout).expect("release-assurance JSON");
    assert_eq!(value["fuzz"]["status"], "ERROR");
    assert_ne!(value["release_status"], "PASS");

    // 2. Legacy or hand-crafted evidence claiming campaign_completed = true
    //    with case_count = 0 must also never score as PASS.
    let mut legacy_zero_ev = incomplete_ev;
    legacy_zero_ev.campaign_completed = true;
    let legacy_path = dir.join("legacy-zero-evidence.json");
    std::fs::write(
        &legacy_path,
        serde_json::to_string_pretty(&legacy_zero_ev).expect("serialize legacy zero evidence"),
    )
    .expect("write legacy zero evidence");

    let out = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        legacy_path.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .code(1);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let value: Value = serde_json::from_str(&stdout).expect("release-assurance JSON");
    assert_eq!(value["fuzz"]["status"], "ERROR");
    assert_ne!(value["release_status"], "PASS");
}

#[test]
fn release_assurance_passes_clean_nonzero_fuzz_evidence() {
    let out_dir = temp_dir("ra-clean");
    sdkt(&[
        "fuzz",
        "campaign",
        &fixture("us_new.wasm"),
        "--cases",
        "2",
        "--artifact-dir",
        out_dir.to_str().unwrap(),
    ])
    .assert()
    .success()
    .code(0);

    let evidence = out_dir.join("campaign-evidence.json");
    let out = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        evidence.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .success()
    .code(0);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let value: Value = serde_json::from_str(&stdout).expect("release-assurance JSON");
    assert_eq!(value["fuzz"]["status"], "PASS");
    assert_eq!(value["fuzz"]["evidence"]["campaign_completed"], true);
    assert_eq!(value["fuzz"]["evidence"]["case_count"], 2);
    assert_eq!(
        value["fuzz"]["detail"],
        format!(
            "fuzz campaign: cases=2 findings=0 minimized={} artifacts={} replayed={} deterministic={}",
            value["fuzz"]["evidence"]["minimized_findings"],
            value["fuzz"]["evidence"]["artifacts_available"],
            value["fuzz"]["evidence"]["replay"].as_object().unwrap().len(),
            value["fuzz"]["evidence"]["deterministic"],
        )
    );
}

#[test]
fn release_assurance_aggregates_multiple_fuzz_evidence_files() {
    let clean_dir = temp_dir("ra-multi-clean");
    sdkt(&[
        "fuzz",
        "campaign",
        &fixture("us_new.wasm"),
        "--cases",
        "2",
        "--artifact-dir",
        clean_dir.to_str().unwrap(),
    ])
    .assert()
    .success();
    let clean = clean_dir.join("campaign-evidence.json");
    let findings_dir = campaign_with_findings();
    let findings = findings_dir.join("campaign-evidence.json");

    let out = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        clean.to_str().unwrap(),
        "--fuzz-evidence",
        findings.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .code(1);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let value: Value = serde_json::from_str(&stdout).expect("release-assurance JSON");
    assert_eq!(value["fuzz"]["status"], "FAIL");
    let detail = value["fuzz"]["detail"].as_str().unwrap();
    assert!(detail.contains(clean.to_str().unwrap()), "{detail}");
    assert!(detail.contains(findings.to_str().unwrap()), "{detail}");

    let clean_copy = clean_dir.join("second-clean-evidence.json");
    std::fs::copy(&clean, &clean_copy).expect("copy second clean evidence file");
    let out = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        clean.to_str().unwrap(),
        "--fuzz-evidence",
        clean_copy.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .success()
    .code(0);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let value: Value = serde_json::from_str(&stdout).expect("release-assurance JSON");
    assert_eq!(value["fuzz"]["status"], "PASS");
}

#[test]
fn release_assurance_incomplete_evidence_dominates_clean_evidence() {
    let clean_dir = temp_dir("ra-incomplete-clean");
    sdkt(&[
        "fuzz",
        "campaign",
        &fixture("us_new.wasm"),
        "--cases",
        "2",
        "--artifact-dir",
        clean_dir.to_str().unwrap(),
    ])
    .assert()
    .success();
    let clean = clean_dir.join("campaign-evidence.json");
    let incomplete_dir = temp_dir("ra-incomplete");
    let incomplete = sdkt_fuzz::CampaignEvidence::from_result(
        &sdkt_fuzz::CampaignResult::default(),
        sdkt_fuzz::CampaignConfig::default(),
        &[],
        &std::collections::BTreeMap::new(),
        true,
    );
    let incomplete_path = incomplete_dir.join("incomplete.json");
    std::fs::write(&incomplete_path, serde_json::to_vec(&incomplete).unwrap()).unwrap();

    let out = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        clean.to_str().unwrap(),
        "--fuzz-evidence",
        incomplete_path.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .code(1);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let value: Value = serde_json::from_str(&stdout).expect("release-assurance JSON");
    assert_eq!(value["fuzz"]["status"], "ERROR");
    let detail = value["fuzz"]["detail"].as_str().unwrap();
    assert!(detail.contains(clean.to_str().unwrap()), "{detail}");
    assert!(
        detail.contains(incomplete_path.to_str().unwrap()),
        "{detail}"
    );
    assert!(
        detail.contains(&format!(
            "{}: fuzz campaign incomplete (campaign_completed=false, cases=0 findings=0 minimized=0)",
            incomplete_path.display(),
        )),
        "{detail}"
    );
}
