//! Integration tests for `sdkt audit --format sarif`.
//!
//! Acceptance criteria covered:
//!  - `--format sarif` emits a valid SARIF 2.1.0 document
//!  - Each finding maps to a result with the correct `level`
//!  - The audited file path appears as the artifact location URI
//!  - Rule ids appear in the `runs[0].tool.driver.rules` array
//!  - The document validates against the pinned SARIF 2.1.0 JSON-schema
//!  - Empty-findings case: valid SARIF with zero results
//!  - Existing pretty/JSON output is unchanged (regression guard)

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

/// Load the pinned SARIF 2.1.0 schema bundled as a test fixture.
fn sarif_schema() -> serde_json::Value {
    let schema_bytes = include_bytes!("fixtures/sarif-2.1.0.json");
    serde_json::from_slice(schema_bytes).expect("fixture is valid JSON")
}

/// Validate a JSON value against the bundled SARIF 2.1.0 schema.
fn assert_valid_sarif(doc: &serde_json::Value) {
    let schema = sarif_schema();
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    if let Err(e) = validator.validate(doc) {
        panic!("SARIF document failed schema validation: {e}");
    }
}

// ── Document structure ────────────────────────────────────────────────────────

#[test]
fn sarif_output_has_version_and_schema_fields() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "bad.rs", "pub fn mint(to: Address) { }\n");
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).expect("sarif output is valid JSON");

    assert_eq!(v["version"], "2.1.0", "SARIF version must be 2.1.0");
    assert!(
        v["$schema"].as_str().unwrap_or("").contains("sarif"),
        "$schema URI must reference sarif"
    );
    assert!(v["runs"].is_array(), "runs array present");
    assert_eq!(v["runs"].as_array().unwrap().len(), 1, "exactly one run");
}

#[test]
fn sarif_output_has_tool_driver() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "ok.rs",
        "pub fn balance(who: Address) -> u32 { require_auth(); 0 }\n",
    );
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let driver = &v["runs"][0]["tool"]["driver"];
    assert_eq!(driver["name"], "sdkt", "tool name is sdkt");
    assert!(driver["version"].is_string(), "tool version is a string");
    assert!(
        driver["informationUri"].is_string(),
        "informationUri is a string"
    );
}

// ── Findings → results mapping ────────────────────────────────────────────────

#[test]
fn sarif_each_finding_becomes_a_result() {
    let dir = TempDir::new().unwrap();
    // AUTH-001 and AUTH-003 both fire on this source
    let path = write_fixture(
        &dir,
        "bad.rs",
        "pub fn initialize(admin: Address) { }\npub fn mint(to: Address) { }\n",
    );
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let results = v["runs"][0]["results"].as_array().expect("results array");
    assert!(
        !results.is_empty(),
        "source with issues must produce results"
    );
    for result in results {
        assert!(result["ruleId"].is_string(), "ruleId present");
        assert!(result["level"].is_string(), "level present");
        assert!(
            result["message"]["text"].is_string(),
            "message.text present"
        );
        assert!(result["locations"].is_array(), "locations array present");
    }
}

#[test]
fn sarif_critical_severity_maps_to_error_level() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "bad.rs", "pub fn mint(to: Address) { }\n");
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let results = v["runs"][0]["results"].as_array().unwrap();
    let auth001 = results.iter().find(|r| r["ruleId"] == "AUTH-001");
    let result = auth001.expect("AUTH-001 finding present");
    assert_eq!(result["level"], "error", "Critical maps to SARIF error");
}

#[test]
fn sarif_warning_severity_maps_to_warning_level() {
    // MOVE-001 is the only Warning-severity built-in rule.
    // It fires when a local is passed as a call argument ≥2 times.
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "move.rs",
        // x is passed to two separate call positions → MOVE-001 fires
        "pub fn do_swap(x: i128) { foo(x); bar(x); }\n",
    );
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let results = v["runs"][0]["results"].as_array().unwrap();
    let move001 = results.iter().find(|r| r["ruleId"] == "MOVE-001");
    let result = move001.expect("MOVE-001 (warning) finding present");
    assert_eq!(
        result["level"], "warning",
        "Warning severity maps to SARIF warning"
    );
}

// ── Artifact location (file path) ─────────────────────────────────────────────

#[test]
fn sarif_artifact_uri_contains_source_file_path() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "mycontract.rs", "pub fn mint(to: Address) { }\n");
    let path_str = path.to_str().unwrap();
    let out = sdkt()
        .args(["audit", path_str, "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let results = v["runs"][0]["results"].as_array().unwrap();
    assert!(!results.is_empty(), "need at least one result to check");

    for result in results {
        let uri = result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .expect("uri is a string");
        // The URI must include the path that was passed on the command line.
        assert!(
            uri.contains("mycontract.rs"),
            "artifact URI should contain the source filename; got: {uri}"
        );
    }
}

// ── Rules section ─────────────────────────────────────────────────────────────

#[test]
fn sarif_rule_ids_appear_in_rules_section() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "bad.rs",
        "pub fn initialize(admin: Address) { }\npub fn mint(to: Address) { }\n",
    );
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let rules = v["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .expect("rules array");
    assert!(!rules.is_empty(), "rules section is non-empty");

    // Every ruleId in results must have a matching entry in the rules array.
    let results = v["runs"][0]["results"].as_array().unwrap();
    let rule_ids_in_rules: Vec<&str> = rules.iter().filter_map(|r| r["id"].as_str()).collect();

    for result in results {
        let rid = result["ruleId"].as_str().unwrap();
        assert!(
            rule_ids_in_rules.contains(&rid),
            "ruleId '{rid}' in results must have an entry in rules section"
        );
    }
}

// ── Empty-findings case ───────────────────────────────────────────────────────

#[test]
fn sarif_empty_findings_produces_valid_document_with_zero_results() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "ok.rs",
        "pub fn balance(who: Address) -> u32 { require_auth(); 0 }\n",
    );
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let results = v["runs"][0]["results"].as_array().expect("results array");
    assert!(
        results.is_empty(),
        "clean source produces zero SARIF results"
    );
    // Rules section should also be empty when there are no findings.
    let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
    assert!(rules.is_empty(), "no rules section for zero findings");
}

// ── JSON-schema validation ────────────────────────────────────────────────────

#[test]
fn sarif_output_validates_against_sarif_2_1_0_schema_with_findings() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "bad.rs",
        "pub fn initialize(admin: Address) { }\npub fn mint(to: Address) { }\n",
    );
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let doc: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    assert_valid_sarif(&doc);
}

#[test]
fn sarif_output_validates_against_sarif_2_1_0_schema_empty_findings() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "ok.rs",
        "pub fn balance(who: Address) -> u32 { require_auth(); 0 }\n",
    );
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let doc: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    assert_valid_sarif(&doc);
}

// ── Regression: existing formats unchanged ────────────────────────────────────

#[test]
fn json_format_still_produces_findings_and_summary_fields() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "bad.rs", "pub fn initialize(admin: Address) { }\n");
    let out = sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    assert!(
        v.get("findings").is_some(),
        "json output must have findings"
    );
    assert!(v.get("summary").is_some(), "json output must have summary");
    // Ensure it does NOT look like SARIF
    assert!(v.get("runs").is_none(), "json output must not be SARIF");
}

#[test]
fn pretty_format_still_produces_text_report() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "bad.rs", "pub fn mint(to: Address) { }\n");
    sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "pretty"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Static Analysis Report"))
        .stdout(predicates::str::contains("AUTH-001"));
}

#[test]
fn invalid_format_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "ok.rs",
        "pub fn balance(who: Address) -> u32 { require_auth(); 0 }\n",
    );
    sdkt()
        .args(["audit", path.to_str().unwrap(), "--format", "xml"])
        .assert()
        .failure();
}

#[test]
fn list_rules_with_sarif_format_exits_nonzero_with_error() {
    sdkt()
        .args(["audit", "--list-rules", "--format", "sarif"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--format sarif"));
}

#[test]
fn sarif_uribaseid_present_for_relative_path() {
    // Write a fixture with a relative path (just a filename in the cwd).
    let rel_name = "_sarif_test_relative_fixture.rs";
    let cwd = std::env::current_dir().unwrap();
    let abs = cwd.join(rel_name);
    std::fs::write(&abs, "pub fn mint(to: Address) { }\n").unwrap();

    let out = sdkt()
        .args(["audit", rel_name, "--format", "sarif"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Best-effort cleanup; ignore errors.
    let _ = std::fs::remove_file(&abs);

    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let results = v["runs"][0]["results"].as_array().unwrap();
    if !results.is_empty() {
        let uri_base =
            &results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uriBaseId"];
        assert_eq!(
            uri_base, "%SRCROOT%",
            "relative path must have %SRCROOT% uriBaseId"
        );
    }
}
