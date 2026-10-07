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
use std::path::PathBuf;

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
    fuzz_campaign(&[
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
    .stdout(predicate::str::contains("RETURN_MISMATCH"));

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
    assert_eq!(value["campaign_completed"], true);
    assert!(value["findings"].as_array().is_some_and(|f| !f.is_empty()));
    assert_eq!(value["deterministic"], true);
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
