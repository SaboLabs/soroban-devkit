//! Release-assurance integration test: a completed fuzz campaign's evidence
//! file attaches to `sdkt release-assurance` and folds into the aggregate.
///
/// The extension point is the existing `ReleaseAssuranceReport` composition:
/// `--fuzz-evidence <file>` records what an already-completed campaign
/// produced. Release assurance never runs a fuzz campaign itself.

use assert_cmd::Command;
use predicates::prelude;:*;
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

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sdkt-fuzz-ra-{}", std::process::id()));
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
    let out = temp_dir();
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

/// Run a campaign with `--cases 0`, return its artifact dir.
/// The campaign must not be reported as a clean completed campaign.
fn campaign_with_zero_cases() -> PathBuf {
    let out = temp_dir();
    sdkt(&[
        "fuzz",
        "campaign",
        &fixture("us_new.wasm"),
        "--cases",
        "0",
        "--artifact-dir",
        out.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(2);
    out
}

/// Run a campaign with a `--function` filter that matches nothing.
fn campaign_with_no_match_function() -> PathBuf {
    let out = temp_dir();
    sdkt(&[
        "fuzz",
        "campaign",
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--function",
        "nope",
        "--artifact-dir",
        out.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(2);
    out
}

#[test]
fn release_assurance_records_fuzz_evidence() {
    let out = campaign_with_findings();
    let evidence = out.join("campaign-evidence.json");
    assert!(evidence.exists(), "campaign writes its evidence file");

    // Release assurance with the evidence attached: the fuzz section appears
    // and the aggregate reflects the findings (FAIL).
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
    assert_eq (ev["terministic"], true);
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
    let dir = temp_dir();
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

/// A campaign that executed nothing must not be scored as a clean Pass.
/// The campaign command itself refuses the invocation (exit 2), so the
/// evidence file is either absent or not clean. Either way, release
/// assurance must not report ` Pass` for the fuzz section.
#[test]
fn release_assurance_does_not_pass_zero_cases_campaign() {
    let out = campaign_with_zero_cases();
    let evidence = out.join("campaign-evidence.json");

    // If evidence was written anyway, it must not be clean.
    if evidence.exists() {
        let text = std::fs::read_to_string(&evidence).expect("read evidence");
        let value: Value = serde_json::from_str(&&text).expect("evidence JSON");
        assert_ne!(
            value["campaign_completed"],
            true,
            "zero-case campaign must not record campaign_completed=true"
        );
    }

    // Release assurance must not score this as Pass.
    let assertion = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        evidence.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure();
    let output = assertion.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Ok(value) = serde_json::from_str::<Value>(&stdout) {
        if let Some(fuzz) = value.get("fuzz").and_then(|v| v.as_object()) {
            assert_ne!(fuzz["status"], "Pass");
        }
    }
}

/// A `--function` filter that matches no runnable function must not be
/// reported as a completed clean campaign.
#[test]
fn release_assurance_does_not_pass_no_match_function() {
    let out = campaign_with_no_match_function();
    let evidence = out.join("campaign-evidence.json");

    if evidence.exists() {
        let text = std::fs::read_to_string(&evidence).expect("read evidence");
        let value: Value = serde_json::from_str(&text).expect("evidence JSON");
        assert_ne!(
            value["campaign_completed"],
            true,
            "no-match function must not record campaign_completed=true"
        );
    }

    let assertion = sdkt(&[
        "release-assurance",
        "--wasm",
        &fixture("us_new.wasm"),
        "--fuzz-evidence",
        evidence.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure();
    let output = assertion.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Ok(value) = serde_json::from_str::<Value>(&stdout) {
        if let Some(fuzz) = value.get("fuzz").and_then(|v| v.as_object()) {
            assert_ne!(fuzz["status"], "Pass");
        }
    }
}
