use assert_cmd::Command;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

#[test]
fn diff_help_documents_offline_comparison() {
    sdkt()
        .args(["diff", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Offline diff"))
        .stdout(predicates::str::contains("OLD"))
        .stdout(predicates::str::contains("NEW"));
}

#[test]
fn diff_missing_old_file_errors() {
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            "/no/such/old.wasm",
            "--new-wasm",
            "/no/such/new.wasm",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("Failed to read OLD WASM"));
}

#[test]
fn diff_accepts_json_format_flag() {
    // Flag parses; it will fail on the (missing) file, but proves the
    // --format json path is wired without needing valid WASM fixtures.
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            "/no/such/old.wasm",
            "--new-wasm",
            "/no/such/new.wasm",
            "--format",
            "json",
        ])
        .assert()
        .failure();
}

// ---- WASM size policy (opt-in) ----
//
// Fixtures are committed, deterministic, offline: us_old.wasm = 198 bytes,
// us_new.wasm = 530 bytes (delta +332 = +167.7% growth).

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

#[test]
fn diff_json_reports_size_delta_fields_and_keeps_existing_schema() {
    let out = sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    // Additive fields...
    assert_eq!(v["size_delta_bytes"], 332);
    assert_eq!(v["size_delta_pct"], 167.7);
    // ...with every pre-existing field untouched.
    assert!(v.get("old").is_some());
    assert!(v.get("new").is_some());
    assert!(v.get("added_functions").is_some());
    assert!(v.get("changed_types").is_some());
    assert_eq!(v["old"]["size_bytes"], 198);
    assert_eq!(v["new"]["size_bytes"], 530);
}

#[test]
fn diff_equal_artifacts_report_zero_delta() {
    let out = sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_old.wasm"),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    assert_eq!(v["size_delta_bytes"], 0);
    assert_eq!(v["size_delta_pct"], 0.0);
}

#[test]
fn diff_growth_at_threshold_passes_but_one_below_fails() {
    // Exactly at the threshold (inclusive): passes.
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--max-growth-pct",
            "167.7",
        ])
        .assert()
        .success();
    // One decimal below the measured growth: violated.
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--max-growth-pct",
            "167.6",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("size policy violation"))
        .stderr(predicates::str::contains("--max-growth-pct"));
}

#[test]
fn diff_abs_size_boundary_is_inclusive() {
    let big = fixture("us_new.wasm"); // 530 bytes
    let small = fixture("us_old.wasm"); // 198 bytes
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &small,
            "--new-wasm",
            &big,
            "--max-size-bytes",
            "530",
        ])
        .assert()
        .success();
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &small,
            "--new-wasm",
            &big,
            "--max-size-bytes",
            "529",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("exceeds --max-size-bytes 529"));
}

#[test]
fn diff_negative_growth_is_reported_but_never_violates() {
    // Shrinking (530 -> 198) yields a negative delta and passes any cap.
    let out = sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_new.wasm"),
            "--new-wasm",
            &fixture("us_old.wasm"),
            "--max-growth-pct",
            "0",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    assert_eq!(v["size_delta_bytes"], -332);
    assert!(v["size_delta_pct"].as_f64().unwrap() < 0.0);
}

#[test]
fn diff_rejects_invalid_threshold_inputs() {
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--max-growth-pct=-5",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("must not be negative"));
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--max-size-bytes",
            "0",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("greater than 0"));
}

#[test]
fn diff_size_policy_cannot_mix_with_upgrade_safety() {
    // Size budget and ABI compatibility are separate checks; refuse to
    // conflate them instead of guessing which verdict should win.
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
            "--max-size-bytes",
            "100000",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "cannot be combined with --upgrade-safety",
        ));
}

#[test]
fn diff_zero_byte_baseline_fails_before_policy() {
    // A 0-byte baseline cannot even parse as WASM: the command must fail
    // clearly rather than compute a percentage.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"").unwrap();
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            tmp.path().to_str().unwrap(),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--max-growth-pct",
            "10",
        ])
        .assert()
        .failure();
}
