//! CLI smoke tests for `sdkt fuzz campaign` / `sdkt fuzz replay`.
//!
//! The fixtures are the repository's committed `us_new.wasm` (counter:
//! `hello()` returns 42, `increment()` bumps instance state) and the
//! purpose-built auth fixture under `sdkt-fuzz/tests/fixtures/`.
//!
//! Findings below are produced by rules declared *in the test
//! (`--expect-success hello`) — known fixture behavior under an explicit
//! oracle declaration. No real-world vulnerability discovery is claimed.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fuzz(args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("built binary");
    cmd.arg("fuzz");
    cmd.args(args);
    cmd
}

/// `sdkt fuzz campaign <wasm> [flags]`
fn fuzz_campaign(args: &[&str]) -> Command {
    let mut all = vec!["campaign"];
    all.extend_from_slice(args);
    fuzz(&all)
}

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_str()
        .expect("utf-8 fixture path")
        .to_string()
}

/// The auth fixture ships with the sdkt-fuzz crate (its tests own it).
fn auth_fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../sdkt-fuzz/tests/fixtures/auth_probe.wasm")
        .to_str()
        .expect("utf-8 fixture path")
        .to_string()
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sdkt-fuzz-cli-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

// ---------------------------------------------------------------------------
// 1. Campaign with a declared rule violated → findings, artifacts, exit 1
// ---------------------------------------------------------------------------

#[test]
fn cli_campaign_finding_exits_one_and_writes_artifacts() {
    let out = temp_dir("finding");
    let output = fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--seed",
        "7",
        "--expect-error",
        "hello:Storage:3",
        "--artifact-dir",
        out.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .code(1)
    .stdout(predicate::str::contains("findings"))
    .stdout(predicate::str::contains("RETURN_MISMATCH"))
    .get_output()
    .stdout
    .clone();
    let stdout: Value = serde_json::from_slice(&output).expect("campaign JSON evidence");
    assert_campaign_provenance(&stdout);

    // Artifacts: one per finding, named deterministically.
    let mut artifacts = std::fs::read_dir(&out)
        .expect("artifact dir readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("finding-") && n.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    artifacts.sort();
    assert!(
        !artifacts.is_empty(),
        "a campaign with findings must write artifacts"
    );
    for path in &artifacts {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with("finding-") {
            continue; // campaign-evidence.json is checked below
        }
        let json = std::fs::read_to_string(path).expect("artifact readable");
        let value: Value = serde_json::from_str(&json).expect("artifact is valid JSON");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["reason_code"], "RETURN_MISMATCH");
        assert_eq!(value["campaign"]["wasm_sha256"].as_str().unwrap().len(), 64);
        assert!(value["sequence"].as_array().is_some_and(|s| !s.is_empty()));
    }

    // Evidence file is written alongside the artifacts.
    let evidence = out.join("campaign-evidence.json");
    let text = std::fs::read_to_string(&evidence).expect("evidence written");
    let value: Value = serde_json::from_str(&text).expect("evidence is JSON");
    assert_campaign_provenance(&value);
    assert_eq!(value["campaign_completed"], true);
    assert!(value["findings"].as_array().is_some_and(|f| !f.is_empty()));
    assert_eq!(value["deterministic"], true);
}

fn assert_campaign_provenance(evidence: &Value) {
    assert_eq!(
        evidence["schema_version"],
        sdkt_fuzz::CAMPAIGN_EVIDENCE_SCHEMA_VERSION
    );
    assert_eq!(evidence["tool"], "sdkt");
    assert_eq!(evidence["version"], env!("CARGO_PKG_VERSION"));
}

// ---------------------------------------------------------------------------
// 2. Clean campaign: no declarations → all PASS, no artifacts, exit 0
// ---------------------------------------------------------------------------

#[test]
fn cli_campaign_clean_exits_zero() {
    let out = temp_dir("clean");
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "2",
        "--artifact-dir",
        out.to_str().unwrap(),
    ])
    .assert()
    .success()
    .code(0)
    .stdout(predicate::str::contains("executed=2"))
    .stdout(predicate::str::contains("findings=0"));

    // No finding artifacts: PASS never becomes an artifact.
    let findings = std::fs::read_dir(&out)
        .expect("artifact dir readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("finding-"))
        })
        .count();
    assert_eq!(findings, 0);
}

#[test]
fn cli_campaign_rejects_conflicting_expectations_for_same_function() {
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "1",
        "--expect-success",
        "hello",
        "--expect-error",
        "hello:Contract:1",
    ])
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains(
        "conflicting expectations for function 'hello'",
    ));
}

#[test]
fn cli_campaign_accepts_single_success_expectation() {
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "2",
        "--function",
        "hello",
        "--expect-success",
        "hello",
    ])
    .assert()
    .success()
    .code(0)
    .stdout(predicate::str::contains("findings=0"));
}

// ---------------------------------------------------------------------------
// 4. Replay of a real artifact: reproduced, exit 0
// ---------------------------------------------------------------------------

#[test]
fn cli_replay_reproduces_and_exits_zero() {
    let out = temp_dir("replay");
    // Produce an artifact first.
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--seed",
        "11",
        "--expect-error",
        "hello:Storage:3",
        "--artifact-dir",
        out.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(1);

    let artifact = std::fs::read_dir(&out)
        .expect("dir readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("finding-") && n.ends_with(".json"))
        })
        .expect("at least one finding artifact");

    fuzz(&[
        "replay",
        artifact.to_str().unwrap(),
        "--wasm",
        &fixture("us_new.wasm"),
    ])
    .assert()
    .success()
    .code(0)
    .stdout(predicate::str::contains("REPRODUCED"))
    .stdout(predicate::str::contains("RETURN_MISMATCH"));
}

// ---------------------------------------------------------------------------
// 5. Replay with the wrong WASM: invalid input, exit 2 (never a finding)
// ---------------------------------------------------------------------------

#[test]
fn cli_replay_wrong_wasm_is_invalid_input() {
    let out = temp_dir("wrongwasm");
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--seed",
        "13",
        "--expect-error",
        "hello:Storage:3",
        "--artifact-dir",
        out.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(1);

    let artifact = std::fs::read_dir(&out)
        .expect("dir readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("finding-") && n.ends_with(".json"))
        })
        .expect("at least one finding artifact");

    fuzz(&[
        "replay",
        artifact.to_str().unwrap(),
        "--wasm",
        &fixture("us_old.wasm"),
    ])
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains("wasm hash mismatch"));
}

// ---------------------------------------------------------------------------
// 6. Replay of a malformed artifact: invalid input, exit 2
// ---------------------------------------------------------------------------

#[test]
fn cli_replay_malformed_artifact_is_invalid_input() {
    let dir = temp_dir("malformed");
    let path = dir.join("finding-broken.json");
    std::fs::write(&path, b"{ this is not json ").expect("write broken artifact");

    fuzz(&[
        "replay",
        path.to_str().unwrap(),
        "--wasm",
        &fixture("us_new.wasm"),
    ])
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains("artifact"));
}

// ---------------------------------------------------------------------------
// 7. Missing WASM file / bad flags: usage error, exit 2
// ---------------------------------------------------------------------------

#[test]
fn cli_campaign_missing_wasm_is_usage_error() {
    fuzz_campaign(&["/nonexistent/nope.wasm", "--cases", "1"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn cli_campaign_bad_format_is_usage_error() {
    fuzz_campaign(&[&fixture("us_new.wasm"), "--cases", "1", "--format", "nope"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid format"));
}

#[test]
fn cli_campaign_bad_auth_mode_is_usage_error() {
    fuzz_campaign(&[&fixture("us_new.wasm"), "--cases", "1", "--auth", "maybe"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid auth mode"));
}

#[test]
fn cli_campaign_zero_cases_is_usage_error() {
    let out = temp_dir("zero-cases");
    let _ = std::fs::remove_dir_all(&out);
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "0",
        "--artifact-dir",
        out.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains("cases must be greater than 0"));

    assert!(
        !out.join("campaign-evidence.json").exists(),
        "zero-case campaign must not write campaign-evidence.json"
    );
}

#[test]
fn cli_campaign_unmatched_function_is_usage_error() {
    let out = temp_dir("unmatched-fn");
    let _ = std::fs::remove_dir_all(&out);
    fuzz_campaign(&[
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
    .code(2)
    .stderr(predicate::str::contains("nope"))
    .stderr(predicate::str::contains("matched no runnable function"));

    assert!(
        !out.join("campaign-evidence.json").exists(),
        "unmatched --function campaign must not write campaign-evidence.json"
    );
}

// ---------------------------------------------------------------------------
// 8. Deterministic campaign stream across runs (same flags, same artifacts)
// ---------------------------------------------------------------------------

#[test]
fn cli_campaign_artifact_stream_is_deterministic() {
    let a = temp_dir("det-a");
    let b = temp_dir("det-b");
    for dir in [&a, &b] {
        fuzz_campaign(&[
            &fixture("us_new.wasm"),
            "--cases",
            "4",
            "--seed",
            "17",
            "--expect-error",
            "hello:Storage:3",
            "--artifact-dir",
            dir.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1);
    }
    let names = |dir: &PathBuf| -> Vec<String> {
        let mut v = std::fs::read_dir(dir)
            .expect("dir readable")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("finding-") && n.ends_with(".json"))
            })
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect::<Vec<_>>();
        v.sort();
        v
    };
    let (na, nb) = (names(&a), names(&b));
    assert!(
        !na.is_empty(),
        "a declared-rule-violating campaign must write finding artifacts"
    );
    assert_eq!(
        na, nb,
        "same flags and seed must produce the same artifact names"
    );
    // Deterministic output: byte-identical artifacts for the same inputs.
    for name in &na {
        let left = std::fs::read(a.join(name)).expect("readable");
        let right = std::fs::read(b.join(name)).expect("readable");
        assert_eq!(
            left, right,
            "artifact {name} must be byte-identical across runs"
        );
    }
}

// ---------------------------------------------------------------------------
// 9. Auth-mode campaigns through the manifest: declarations classify
// ---------------------------------------------------------------------------

#[test]
fn cli_campaign_noauth_with_declared_auth_rule_is_clean_run() {
    // The auth fixture exports `bump(Address)` (require_auth), `set(u32)` and
    // `peek()`. Declaring the Auth/6 error for `bump` makes every NoAuth
    // auth failure EXPECTED_ERROR — a declared, clean run with no findings.
    let out = temp_dir("auth");
    fuzz_campaign(&[
        &auth_fixture(),
        "--cases",
        "6",
        "--expect-error",
        "bump:Auth:6",
        "--artifact-dir",
        out.to_str().unwrap(),
    ])
    .assert()
    .success()
    .code(0)
    .stdout(predicate::str::contains("findings=0"))
    .stdout(predicate::str::contains("expected_errors="));
}

#[test]
fn cli_campaign_wrong_auth_guarded_success_rule_reports_findings() {
    // Under wrong-auth, the guarded `bump` cannot authenticate; declaring
    // that it must succeed makes the failure an UNEXPECTED_ERROR finding.
    let out = temp_dir("wrongauth");
    fuzz_campaign(&[
        &auth_fixture(),
        "--cases",
        "4",
        "--auth",
        "wrong-auth",
        "--expect-success",
        "bump",
        "--artifact-dir",
        out.to_str().unwrap(),
        "--format",
        "json",
    ])
    .assert()
    .failure()
    .code(1)
    .stdout(predicate::str::contains("UNEXPECTED_ERROR"));

    // The artifact records the auth mode used.
    let artifact = std::fs::read_dir(&out)
        .expect("dir readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("finding-") && n.ends_with(".json"))
        })
        .expect("finding artifact");
    let value: Value = serde_json::from_str(&std::fs::read_to_string(&artifact).unwrap()).unwrap();
    assert_eq!(value["auth_mode"], "wrong_auth");
    // And it replays.
    fuzz(&[
        "replay",
        artifact.to_str().unwrap(),
        "--wasm",
        &auth_fixture(),
    ])
    .assert()
    .success()
    .code(0)
    .stdout(predicate::str::contains("REPRODUCED"));
}

// ---------------------------------------------------------------------------
// Issue #536: `sdkt fuzz compare` — new / fixed / persisting findings
// ---------------------------------------------------------------------------

use sdkt_fuzz::evidence::FindingEvidence;
use sdkt_fuzz::{CampaignConfig, CampaignEvidence};
use std::collections::BTreeMap;

fn compare_evidence_file(
    dir: &std::path::Path,
    name: &str,
    wasm_sha256: &str,
    findings: Vec<(&str, &str, &str, &str)>,
) -> PathBuf {
    let evidence = CampaignEvidence {
        campaign_completed: true,
        config: CampaignConfig {
            wasm_sha256: wasm_sha256.to_string(),
            ..CampaignConfig::default()
        },
        case_count: findings.len(),
        passed: 0,
        expected_errors: 0,
        findings: findings
            .into_iter()
            .map(
                |(case_id, function, reason_code, artifact_hash)| FindingEvidence {
                    case_id: case_id.to_string(),
                    function: function.to_string(),
                    reason_code: reason_code.to_string(),
                    artifact_hash: artifact_hash.to_string(),
                    minimized: false,
                },
            )
            .collect(),
        minimized_findings: 0,
        artifacts_available: false,
        replay: BTreeMap::new(),
        deterministic: true,
        schema_version: sdkt_fuzz::CAMPAIGN_EVIDENCE_SCHEMA_VERSION,
        tool: "sdkt".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_string_pretty(&evidence).unwrap()).unwrap();
    path
}

fn fuzz_compare(baseline: &Path, new: &Path, extra: &[&str]) -> Command {
    let mut args = vec![
        "compare",
        "--baseline",
        baseline.to_str().unwrap(),
        "--new",
        new.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    fuzz(&args)
}

#[test]
fn cli_fuzz_compare_new_finding_exits_one() {
    let dir = temp_dir("compare-new");
    let baseline = compare_evidence_file(
        &dir,
        "baseline.json",
        "aaaa",
        vec![("case0001", "increment", "RETURN_MISMATCH", "h1")],
    );
    let new = compare_evidence_file(
        &dir,
        "new.json",
        "aaaa",
        vec![
            ("case0001", "increment", "RETURN_MISMATCH", "h1"),
            ("case0003", "decrement", "RETURN_MISMATCH", "h3"),
        ],
    );

    fuzz_compare(&baseline, &new, &[])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains(
            "New: 1 | Fixed: 0 | Persisting: 1",
        ))
        .stdout(predicate::str::contains("h3"));
}

#[test]
fn cli_fuzz_compare_all_fixed_exits_zero() {
    let dir = temp_dir("compare-fixed");
    let baseline = compare_evidence_file(
        &dir,
        "baseline.json",
        "aaaa",
        vec![("case0001", "increment", "RETURN_MISMATCH", "h1")],
    );
    let new = compare_evidence_file(&dir, "new.json", "aaaa", vec![]);

    fuzz_compare(&baseline, &new, &[])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains(
            "New: 0 | Fixed: 1 | Persisting: 0",
        ));
}

#[test]
fn cli_fuzz_compare_json_output() {
    let dir = temp_dir("compare-json");
    let baseline = compare_evidence_file(
        &dir,
        "baseline.json",
        "aaaa",
        vec![("case0001", "increment", "RETURN_MISMATCH", "h1")],
    );
    let new = compare_evidence_file(
        &dir,
        "new.json",
        "aaaa",
        vec![
            ("case0001", "increment", "RETURN_MISMATCH", "h1"),
            ("case0003", "decrement", "RETURN_MISMATCH", "h3"),
        ],
    );

    let output = fuzz_compare(&baseline, &new, &["--format", "json"])
        .assert()
        .failure()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(v["baseline_wasm_sha256"], "aaaa");
    assert_eq!(v["new_wasm_sha256"], "aaaa");
    assert_eq!(v["new"].as_array().unwrap().len(), 1);
    assert_eq!(v["new"][0]["artifact_hash"], "h3");
    assert_eq!(v["new"][0]["case_id"], "case0003");
    assert_eq!(v["fixed"].as_array().unwrap().len(), 0);
    assert_eq!(v["persisting"].as_array().unwrap().len(), 1);
    assert_eq!(v["persisting"][0]["artifact_hash"], "h1");
}

#[test]
fn cli_fuzz_compare_wasm_mismatch_exits_two() {
    let dir = temp_dir("compare-mismatch");
    let baseline = compare_evidence_file(&dir, "baseline.json", "aaaa", vec![]);
    let new = compare_evidence_file(&dir, "new.json", "bbbb", vec![]);

    fuzz_compare(&baseline, &new, &[])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("aaaa"))
        .stderr(predicate::str::contains("bbbb"));
}

#[test]
fn cli_fuzz_compare_unreadable_file_exits_two() {
    let dir = temp_dir("compare-missing");
    let missing = dir.join("no-such-baseline.json");
    let new = compare_evidence_file(&dir, "new.json", "aaaa", vec![]);

    fuzz_compare(&missing, &new, &[])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("no-such-baseline.json"));
}

// ---------------------------------------------------------------------------
// 10. Sequence execution flags: validation, multi-step campaign + replay,
//     determinism, and default-path parity
// ---------------------------------------------------------------------------

/// Build a stateful counter WASM from `us_new.wasm` whose `increment()`
/// succeeds on the first call (`0 -> 1`) and traps when the carried counter
/// reaches `1` on the second call (`1 == 1 -> unreachable`).
fn write_stateful_sequence_wasm(dir: &Path) -> PathBuf {
    let raw = std::fs::read(fixture("us_new.wasm")).expect("read us_new.wasm");
    let needle = [0x42, 0xff, 0xff, 0xff, 0xff, 0x0f];
    let replacement = [0x42, 0x81, 0x80, 0x80, 0x80, 0x00];
    let mut patched = raw;
    let pos = patched
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("overflow constant present in us_new.wasm");
    patched[pos..pos + needle.len()].copy_from_slice(&replacement);
    let path = dir.join("stateful_sequence.wasm");
    std::fs::write(&path, patched).expect("write stateful wasm");
    path
}

#[test]
fn cli_campaign_rejects_single_sequence_flag_before_starting_work() {
    let base = temp_dir("seq-missing-pair");
    let out_len_only = base.join("should-not-exist-len");
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--sequence-length",
        "3",
        "--artifact-dir",
        out_len_only.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains(
        "--sequence-length requires --sequence-every",
    ));
    assert!(
        !out_len_only.exists(),
        "validation must fail before creating the artifact directory"
    );

    let out_every_only = base.join("should-not-exist-every");
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--sequence-every",
        "2",
        "--artifact-dir",
        out_every_only.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains(
        "--sequence-every requires --sequence-length",
    ));
    assert!(
        !out_every_only.exists(),
        "validation must fail before creating the artifact directory"
    );
}

#[test]
fn cli_campaign_rejects_sequence_bounds_below_minimum() {
    let base = temp_dir("seq-bad-bounds");
    let out_len_one = base.join("should-not-exist-len1");
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--sequence-length",
        "1",
        "--sequence-every",
        "2",
        "--artifact-dir",
        out_len_one.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains(
        "invalid --sequence-length 1: must be >= 2",
    ));
    assert!(!out_len_one.exists());

    let out_every_zero = base.join("should-not-exist-every0");
    fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--sequence-length",
        "3",
        "--sequence-every",
        "0",
        "--artifact-dir",
        out_every_zero.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains(
        "invalid --sequence-every 0: must be >= 1",
    ));
    assert!(!out_every_zero.exists());
}

#[test]
fn cli_campaign_sequence_mode_produces_multistep_artifacts_that_replay_and_are_deterministic() {
    let base = temp_dir("seq-run");
    let wasm = write_stateful_sequence_wasm(&base);
    let out_a = base.join("run-a");
    let out_b = base.join("run-b");

    for out in [&out_a, &out_b] {
        fuzz_campaign(&[
            wasm.to_str().unwrap(),
            "--cases",
            "4",
            "--seed",
            "7",
            "--function",
            "increment",
            "--expect-success",
            "increment",
            "--sequence-length",
            "3",
            "--sequence-every",
            "2",
            "--artifact-dir",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains(
            "Campaign summary: executed=4 passed=2 expected_errors=0 findings=2 skipped=0 sequences=2 single_step=2",
        ))
        .stdout(predicate::str::contains("FINDING case0000 increment UNEXPECTED_ERROR"))
        .stdout(predicate::str::contains("FINDING case0002 increment UNEXPECTED_ERROR"))
        .stdout(predicate::str::contains("deterministic=true"));
    }

    // Evidence records sequence settings and determinism verdict.
    let evidence_a = std::fs::read_to_string(out_a.join("campaign-evidence.json")).unwrap();
    let evidence_b = std::fs::read_to_string(out_b.join("campaign-evidence.json")).unwrap();
    assert_eq!(
        evidence_a, evidence_b,
        "same seed and sequence flags must produce byte-identical campaign evidence"
    );
    let ev: Value = serde_json::from_str(&evidence_a).unwrap();
    assert_eq!(ev["config"]["sequence_length"], 3);
    assert_eq!(ev["config"]["sequence_every"], 2);
    assert_eq!(ev["case_count"], 4);
    assert_eq!(ev["passed"], 2);
    assert_eq!(ev["findings"].as_array().unwrap().len(), 2);
    assert_eq!(ev["deterministic"], true);

    // Artifacts on disk carry the multi-step sequence and replay cleanly.
    for case_id in ["case0000", "case0002"] {
        let name = format!("finding-{case_id}-unexpected_error.json");
        let path_a = out_a.join(&name);
        let path_b = out_b.join(&name);
        let bytes_a = std::fs::read(&path_a).expect("artifact A readable");
        let bytes_b = std::fs::read(&path_b).expect("artifact B readable");
        assert_eq!(
            bytes_a, bytes_b,
            "artifact {name} must be byte-identical across runs"
        );

        let art: Value = serde_json::from_slice(&bytes_a).expect("valid artifact JSON");
        assert_eq!(art["campaign"]["sequence_length"], 3);
        assert_eq!(art["campaign"]["sequence_every"], 2);
        let seq = art["sequence"].as_array().expect("sequence array");
        assert_eq!(
            seq.len(),
            2,
            "multi-step finding artifact must record the 2-step state-carrying sequence"
        );
        assert_eq!(seq[0]["function"], "increment");
        assert_eq!(seq[1]["function"], "increment");

        fuzz(&[
            "replay",
            path_a.to_str().unwrap(),
            "--wasm",
            wasm.to_str().unwrap(),
        ])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("REPRODUCED"))
        .stdout(predicate::str::contains("reason=UNEXPECTED_ERROR"));
    }
}

#[test]
fn cli_campaign_default_path_parity_when_sequence_flags_omitted() {
    let out = temp_dir("default-parity");
    let assert = fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "4",
        "--seed",
        "7",
        "--expect-error",
        "hello:Storage:3",
        "--artifact-dir",
        out.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .code(1);

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let summary_line = stdout
        .lines()
        .find(|l| l.starts_with("Campaign summary:"))
        .expect("Campaign summary line present");
    assert_eq!(
        summary_line,
        "Campaign summary: executed=4 passed=2 expected_errors=0 findings=2 skipped=0",
        "default summary line must stay byte-identical when sequence flags are omitted"
    );

    let ev_text = std::fs::read_to_string(out.join("campaign-evidence.json")).unwrap();
    let ev: Value = serde_json::from_str(&ev_text).unwrap();
    assert_eq!(ev["config"]["sequence_length"], 2);
    assert_eq!(ev["config"]["sequence_every"], 0);
}

// ---------------------------------------------------------------------------
// `--dry-run`: plan without executing (issue #511)
// ---------------------------------------------------------------------------

fn dry_run_json(args: &[&str]) -> serde_json::Value {
    let owned = fixture("us_new.wasm");
    let mut all = vec![owned.as_str()];
    all.extend_from_slice(args);
    let out = fuzz_campaign(&all)
        .args(["--dry-run", "--format", "json"])
        .output()
        .expect("run sdkt");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("plan is json")
}

#[test]
fn dry_run_is_deterministic_for_same_seed() {
    let a = fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--seed",
        "7",
        "--cases",
        "12",
        "--dry-run",
    ])
    .output()
    .unwrap();
    let b = fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--seed",
        "7",
        "--cases",
        "12",
        "--dry-run",
    ])
    .output()
    .unwrap();
    assert_eq!(a.status.code(), Some(0));
    assert_eq!(a.stdout, b.stdout, "pretty plans must be byte-identical");
    let ja = dry_run_json(&["--seed", "7", "--cases", "12"]);
    let jb = dry_run_json(&["--seed", "7", "--cases", "12"]);
    assert_eq!(ja, jb);
}

#[test]
fn dry_run_case_count_equals_cases() {
    for n in [1usize, 5, 17] {
        let plan = dry_run_json(&["--cases", &n.to_string()]);
        assert_eq!(plan["cases"].as_array().unwrap().len(), n);
    }
}

#[test]
fn dry_run_functions_are_selectable_and_filter_applies() {
    let plan = dry_run_json(&["--cases", "9"]);
    let fns: std::collections::BTreeSet<String> = plan["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["function"].as_str().unwrap().to_string())
        .collect();
    assert!(!fns.is_empty());
    let wasm = std::fs::read(fixture("us_new.wasm")).unwrap();
    let input = sdkt_fuzz::CampaignInput::new(&wasm, [0u8; 32]).unwrap();
    let (selectable, _) = input.selectable();
    for f in &fns {
        assert!(selectable.contains(f), "{f} not selectable");
    }
    let only = selectable[0].clone();
    let filtered = dry_run_json(&["--cases", "4", "--function", &only]);
    for c in filtered["cases"].as_array().unwrap() {
        assert_eq!(c["function"], only.as_str());
        assert!(c["auth_mode"].is_string());
        assert!(c["operators"].is_array());
    }
}

#[test]
fn dry_run_unknown_function_errors_like_a_real_run() {
    let wasm = fixture("us_new.wasm");
    let dry = fuzz_campaign(&[&wasm, "--function", "no_such_fn", "--dry-run"])
        .output()
        .unwrap();
    let real = fuzz_campaign(&[&wasm, "--function", "no_such_fn"])
        .output()
        .unwrap();
    assert_eq!(dry.status.code(), Some(2));
    assert_eq!(dry.status.code(), real.status.code());
    assert_eq!(dry.stderr, real.stderr);
}

#[test]
fn dry_run_writes_no_artifacts() {
    let dir = temp_dir("dry-run-artifacts");
    let target = dir.join("artifacts");
    let out = fuzz_campaign(&[
        &fixture("us_new.wasm"),
        "--cases",
        "6",
        "--expect-success",
        "hello",
        "--artifact-dir",
        target.to_str().unwrap(),
        "--dry-run",
    ])
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(!target.exists(), "dry run must not create the artifact dir");
    let _ = std::fs::remove_dir_all(&dir);
}
