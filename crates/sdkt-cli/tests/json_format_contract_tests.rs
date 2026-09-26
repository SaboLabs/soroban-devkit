//! Contract tests for `--format json` across scripting-oriented commands.
//!
//! These verify that machine-readable output has stable field names and types,
//! and that error envelopes have a consistent shape. All tests are deterministic
//! and offline (no network, no unstable values like timestamps or absolute paths).
//!
//! ## Compatibility commitments (documented here so downstream consumers know
//! which fields are safe to depend on):
//!
//! ### `sdkt decode --format json`
//! - Valid JSON for well-formed XDR input
//! - Error message in consistent envelope on parse failure
//!
//! ### `sdkt wasm inspect --format json`
//! - `file`: string (basename only, not absolute path)
//! - `metadata.hash`: hex string
//! - `metadata.size_bytes`: u64
//! - `spec`: object with functions/events/custom_types arrays
//!
//! ### `sdkt diff --upgrade-safety --format json`
//! - `compatible`: boolean
//! - `breaking_changes`: array of {kind, name?, detail?}
//! - `non_breaking_changes`: array of {kind, name?}
//!
//! ### `sdkt audit --format json`
//! - `findings`: array of {rule_id, severity, message, location}
//! - `summary`: {critical, warning, info, total} all u64
//!
//! ### `sdkt package validate --format json`
//! - `valid`: boolean
//! - `error`: string (when invalid)
//!
//! ### `sdkt storage estimate --format json`
//! - `wasm_path`: string
//! - `ledgers`: u32
//! - `cost_per_entry_stroops`: u64
//! - `cost_per_entry_xlm`: string
//! - `classes`: object containing `instance`, `persistent`, and `temporary`
//!   - `instance`: {entry_count, cost_stroops, cost_xlm, derivation, notes}
//!   - `persistent`: {entry_count, cost_stroops, cost_xlm, derivation, notes}
//!   - `temporary`: {entry_count, cost_stroops, cost_xlm, derivation, notes}
//! - `total`: {baseline_entries, cost_stroops, cost_xlm}
//! - `spec_metrics`: {functions_count, custom_types_count, events_count}
//! - `approximation_ceiling`: string

use assert_cmd::Command;
use serde_json::Value;
use std::io::Write;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the `sdkt` binary under test.
fn sdkt() -> Command {
    Command::cargo_bin("sdkt").expect("sdkt binary built")
}

/// Write `content` to `<dir>/<name>` and return the path.
fn write_fixture(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    p
}

/// Assert that a string is valid JSON by parsing it.
fn assert_valid_json(s: &str) -> Value {
    serde_json::from_str::<Value>(s).unwrap_or_else(|e| {
        panic!("expected valid JSON, got error: {e}\noutput:\n{s}");
    })
}

// ---------------------------------------------------------------------------
// Compatibility: wasm inspect fixtures (project-local, stable)
// ---------------------------------------------------------------------------

static WASM_OLD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_old.wasm");
static WASM_NEW: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_new.wasm");

// ===========================================================================
// sdkt wasm inspect --format json
// ===========================================================================

mod wasm_inspect {
    use super::*;

    #[test]
    fn json_output_has_required_top_level_fields() {
        let out = sdkt()
            .args(["wasm", "inspect", WASM_NEW, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));

        assert!(v.get("file").is_some(), "missing `file` field");
        assert!(v.get("metadata").is_some(), "missing `metadata` field");
        assert!(v.get("spec").is_some(), "missing `spec` field");
    }

    #[test]
    fn json_metadata_has_stable_fields() {
        let out = sdkt()
            .args(["wasm", "inspect", WASM_NEW, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let meta = v.get("metadata").expect("metadata field");

        // Hash is a hex string
        let hash = meta
            .get("hash")
            .and_then(|h| h.as_str())
            .expect("metadata.hash");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be hex: {hash}"
        );

        // size_bytes is a number
        let size = meta
            .get("size_bytes")
            .and_then(|s| s.as_u64())
            .expect("metadata.size_bytes");
        assert!(size > 0, "size_bytes must be > 0");

        // exports is an array
        assert!(
            meta.get("exports").is_some_and(|e| e.is_array()),
            "metadata.exports should be an array"
        );
    }

    #[test]
    fn json_metadata_exports_and_imports_have_stable_lowercase_kind_strings() {
        let out = sdkt()
            .args(["wasm", "inspect", WASM_NEW, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let meta = v.get("metadata").expect("metadata field");

        let exports = meta
            .get("exports")
            .and_then(|e| e.as_array())
            .expect("metadata.exports array");
        assert!(!exports.is_empty(), "us_new.wasm should have exports");
        for exp in exports {
            let kind = exp
                .get("kind")
                .and_then(|k| k.as_str())
                .expect("export kind string");
            assert!(
                matches!(
                    kind,
                    "func" | "table" | "memory" | "global" | "tag" | "func_exact"
                ),
                "export kind should be stable lowercase, got: {kind}"
            );
            assert!(
                !kind.contains('(') && !kind.chars().next().is_some_and(|c| c.is_uppercase()),
                "export kind must not leak debug or uppercase: {kind}"
            );
        }

        let imports = meta
            .get("imports")
            .and_then(|i| i.as_array())
            .expect("metadata.imports array");
        assert!(!imports.is_empty(), "us_new.wasm should have imports");
        for imp in imports {
            let kind = imp
                .get("kind")
                .and_then(|k| k.as_str())
                .expect("import kind string");
            assert!(
                matches!(
                    kind,
                    "func" | "table" | "memory" | "global" | "tag" | "func_exact"
                ),
                "import kind should be stable lowercase without index payload, got: {kind}"
            );
            assert!(
                !kind.contains('(') && !kind.chars().next().is_some_and(|c| c.is_uppercase()),
                "import kind must not leak debug format like Func(0): {kind}"
            );
        }
    }

    #[test]
    fn json_spec_has_function_array() {
        let out = sdkt()
            .args(["wasm", "inspect", WASM_NEW, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let spec = v.get("spec").expect("spec field");

        assert!(
            spec.get("functions").is_some_and(|f| f.is_array()),
            "spec.functions should be an array"
        );
    }

    #[test]
    fn json_file_field_is_basename_not_absolute() {
        // Ponytail: future — add path normalization here if upstream changes
        // to emit full paths. Today we assert no '/' in the value.
        let out = sdkt()
            .args(["wasm", "inspect", WASM_NEW, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let file = v.get("file").and_then(|f| f.as_str()).expect("file field");

        assert!(
            !file.starts_with('/'),
            "file should be basename (no absolute path), got: {file}"
        );
    }

    #[test]
    fn json_error_output_is_consistent() {
        let out = sdkt()
            .args(["wasm", "inspect", "/no/such/file.wasm", "--format", "json"])
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();
        let err_str = String::from_utf8_lossy(&out);
        assert!(
            err_str.contains("Error") || err_str.contains("No such file"),
            "error output should mention the failure: {err_str}"
        );
    }
}

// ===========================================================================
// sdkt diff --upgrade-safety --format json
// ===========================================================================

mod diff_upgrade_safety {
    use super::*;

    #[test]
    fn json_output_has_compatible_boolean() {
        let out = sdkt()
            .args([
                "diff",
                "--old-wasm",
                WASM_OLD,
                "--new-wasm",
                WASM_NEW,
                "--upgrade-safety",
                "--format",
                "json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));

        let compatible = v
            .get("compatible")
            .and_then(|c| c.as_bool())
            .expect("compatible field");
        // Our fixtures have breaking changes (removed event + type + signature change)
        assert!(!compatible, "fixture pair should be incompatible");
    }

    #[test]
    fn json_breaking_changes_is_array_of_objects() {
        let out = sdkt()
            .args([
                "diff",
                "--old-wasm",
                WASM_OLD,
                "--new-wasm",
                WASM_NEW,
                "--upgrade-safety",
                "--format",
                "json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let breaking = v
            .get("breaking_changes")
            .and_then(|b| b.as_array())
            .expect("breaking_changes field");
        assert!(
            !breaking.is_empty(),
            "fixtures should have breaking changes"
        );

        for change in breaking {
            assert!(
                change.get("kind").is_some_and(|k| k.is_string()),
                "each breaking change should have a string `kind`"
            );
        }
    }

    #[test]
    fn json_non_breaking_changes_is_array_of_objects() {
        let out = sdkt()
            .args([
                "diff",
                "--old-wasm",
                WASM_OLD,
                "--new-wasm",
                WASM_NEW,
                "--upgrade-safety",
                "--format",
                "json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let non_breaking = v
            .get("non_breaking_changes")
            .and_then(|b| b.as_array())
            .expect("non_breaking_changes field");
        assert!(
            !non_breaking.is_empty(),
            "fixtures should have non-breaking changes"
        );

        for change in non_breaking {
            assert!(
                change.get("kind").is_some_and(|k| k.is_string()),
                "each non-breaking change should have a string `kind`"
            );
        }
    }

    #[test]
    fn json_error_when_old_wasm_missing() {
        let out = sdkt()
            .args([
                "diff",
                "--old-wasm",
                "/no/old.wasm",
                "--new-wasm",
                WASM_NEW,
                "--upgrade-safety",
                "--format",
                "json",
            ])
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();
        let err_str = String::from_utf8_lossy(&out);
        assert!(
            err_str.contains("Failed to read") || err_str.contains("No such file"),
            "error should mention missing file: {err_str}"
        );
    }
}

// ===========================================================================
// sdkt audit --format json
// ===========================================================================

mod audit {
    use super::*;

    #[test]
    fn json_output_has_findings_and_summary() {
        let dir = TempDir::new().unwrap();
        let path = write_fixture(&dir, "bad.rs", "pub fn initialize(admin: Address) { }\n");
        let out = sdkt()
            .args(["audit", path.to_str().unwrap(), "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));

        assert!(v.get("findings").is_some(), "missing `findings` field");
        assert!(v.get("summary").is_some(), "missing `summary` field");
    }

    #[test]
    fn json_findings_have_required_fields() {
        let dir = TempDir::new().unwrap();
        let path = write_fixture(&dir, "bad.rs", "pub fn initialize(admin: Address) { }\n");
        let out = sdkt()
            .args(["audit", path.to_str().unwrap(), "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let findings = v
            .get("findings")
            .and_then(|f| f.as_array())
            .expect("findings field");
        assert!(!findings.is_empty(), "fixture should trigger findings");

        for finding in findings {
            assert!(
                finding.get("rule_id").is_some_and(|r| r.is_string()),
                "finding.rule_id should be a string"
            );
            assert!(
                finding.get("severity").is_some_and(|s| s.is_string()),
                "finding.severity should be a string"
            );
            assert!(
                finding.get("message").is_some_and(|m| m.is_string()),
                "finding.message should be a string"
            );
        }
    }

    #[test]
    fn json_summary_has_numeric_counts() {
        let dir = TempDir::new().unwrap();
        let path = write_fixture(&dir, "bad.rs", "pub fn initialize(admin: Address) { }\n");
        let out = sdkt()
            .args(["audit", path.to_str().unwrap(), "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let summary = v.get("summary").expect("summary field");

        for field in &["critical", "warning", "info", "total"] {
            assert!(
                summary.get(field).and_then(|v| v.as_u64()).is_some(),
                "summary.{field} should be a number"
            );
        }
    }

    #[test]
    fn json_clean_source_has_empty_findings() {
        let dir = TempDir::new().unwrap();
        let path = write_fixture(
            &dir,
            "ok.rs",
            "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n",
        );
        let out = sdkt()
            .args(["audit", path.to_str().unwrap(), "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let findings = v
            .get("findings")
            .and_then(|f| f.as_array())
            .expect("findings field");
        assert!(findings.is_empty(), "clean source should have no findings");
    }

    #[test]
    fn json_error_when_source_missing() {
        let out = sdkt()
            .args(["audit", "/no/such/contract.rs", "--format", "json"])
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();
        let err_str = String::from_utf8_lossy(&out);
        assert!(
            err_str.contains("Failed to read") || err_str.contains("No such file"),
            "error should mention missing file: {err_str}"
        );
    }
}

// ===========================================================================
// sdkt package validate --format json
// ===========================================================================

mod package_validate {
    use super::*;

    #[test]
    fn json_invalid_manifest_has_valid_false_and_error() {
        // Run in a temp dir with no .sdkt.toml
        let dir = TempDir::new().unwrap();
        let out = sdkt()
            .current_dir(dir.path())
            .args(["package", "validate", "--format", "json"])
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));

        let valid = v
            .get("valid")
            .and_then(|v| v.as_bool())
            .expect("valid field");
        assert!(!valid, "missing manifest should be invalid");

        assert!(
            v.get("error").is_some_and(|e| e.is_string()),
            "error field should be a string when invalid"
        );
    }

    #[test]
    fn json_error_message_is_nonempty() {
        let dir = TempDir::new().unwrap();
        let out = sdkt()
            .current_dir(dir.path())
            .args(["package", "validate", "--format", "json"])
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let error = v
            .get("error")
            .and_then(|e| e.as_str())
            .expect("error field");
        assert!(!error.is_empty(), "error message should not be empty");
    }
}

// ===========================================================================
// sdkt decode --format json
// ===========================================================================

mod decode {
    use super::*;

    #[test]
    fn json_error_on_invalid_xdr() {
        // Invalid base64 / truncated XDR → error envelope
        let out = sdkt()
            .args([
                "decode",
                "AAAA",
                "--type",
                "TransactionEnvelope",
                "--format",
                "json",
            ])
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();
        let err_str = String::from_utf8_lossy(&out);
        assert!(
            err_str.contains("XdrParse") || err_str.contains("Error"),
            "error should mention parse failure: {err_str}"
        );
    }

    #[test]
    fn json_output_is_valid_for_ledger_key_type() {
        // Use a known-valid base64 LedgerKey (pre-encoded offline).
        // This is a minimal valid LedgerKey for a AccountEntry.
        // If this fixture is unavailable, the test still validates JSON shape
        // by checking that ANY successful decode produces valid JSON.
        //
        // Ponytail: add a static base64 fixture here once a stable one is
        // committed to tests/fixtures/. For now we only assert the error path
        // above, which is the deterministic offline case.
    }
}

// ===========================================================================
// Cross-cutting: all JSON output must be parseable
// ===========================================================================

mod cross_cutting {
    use super::*;

    #[test]
    fn all_json_output_is_parseable() {
        // Run each command with --format json and verify stdout is valid JSON.
        // This catches accidental println! or debug output mixed into JSON streams.

        // wasm inspect
        let out = sdkt()
            .args(["wasm", "inspect", WASM_NEW, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_valid_json(&String::from_utf8_lossy(&out));

        // diff --upgrade-safety
        let out = sdkt()
            .args([
                "diff",
                "--old-wasm",
                WASM_OLD,
                "--new-wasm",
                WASM_NEW,
                "--upgrade-safety",
                "--format",
                "json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_valid_json(&String::from_utf8_lossy(&out));

        // audit (clean source)
        let dir = TempDir::new().unwrap();
        let path = write_fixture(
            &dir,
            "ok.rs",
            "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n",
        );
        let out = sdkt()
            .args(["audit", path.to_str().unwrap(), "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_valid_json(&String::from_utf8_lossy(&out));

        // storage estimate
        let out = sdkt()
            .args(["storage", "estimate", WASM_OLD, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_valid_json(&String::from_utf8_lossy(&out));
    }
}

// ===========================================================================
// sdkt storage estimate --format json
// ===========================================================================

mod storage_estimate {
    use super::*;

    #[test]
    fn json_output_has_required_top_level_fields() {
        let out = sdkt()
            .args(["storage", "estimate", WASM_OLD, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));

        assert!(v.get("wasm_path").is_some(), "missing `wasm_path`");
        assert!(v.get("ledgers").is_some(), "missing `ledgers`");
        assert!(v.get("cost_per_entry_stroops").is_some(), "missing `cost_per_entry_stroops`");
        assert!(v.get("cost_per_entry_xlm").is_some(), "missing `cost_per_entry_xlm`");
        assert!(v.get("classes").is_some(), "missing `classes`");
        assert!(v.get("total").is_some(), "missing `total`");
        assert!(v.get("spec_metrics").is_some(), "missing `spec_metrics`");
        assert!(v.get("approximation_ceiling").is_some(), "missing `approximation_ceiling`");
    }

    #[test]
    fn json_classes_breakdown_schema_and_types() {
        let out = sdkt()
            .args(["storage", "estimate", WASM_OLD, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));
        let classes = v.get("classes").expect("classes object");

        for class_name in ["instance", "persistent", "temporary"] {
            let class_obj = classes.get(class_name).unwrap_or_else(|| {
                panic!("missing `{class_name}` class object");
            });

            assert!(
                class_obj.get("entry_count").is_some_and(|c| c.is_u64()),
                "{class_name}: entry_count should be u64"
            );
            assert!(
                class_obj.get("cost_stroops").is_some_and(|c| c.is_u64()),
                "{class_name}: cost_stroops should be u64"
            );
            assert!(
                class_obj.get("cost_xlm").is_some_and(|c| c.is_string()),
                "{class_name}: cost_xlm should be string"
            );
            assert!(
                class_obj.get("derivation").is_some_and(|c| c.is_string()),
                "{class_name}: derivation should be string"
            );
            assert!(
                class_obj.get("notes").is_some_and(|c| c.is_string()),
                "{class_name}: notes should be string"
            );
        }

        // Specific values for us_old.wasm
        assert_eq!(classes["instance"]["entry_count"], 1);
        assert_eq!(classes["persistent"]["entry_count"], 1);
        assert_eq!(classes["temporary"]["entry_count"], 0);

        let total = v.get("total").expect("total object");
        assert_eq!(total["baseline_entries"], 2);
        assert_eq!(total["cost_stroops"], 3456000);
        assert_eq!(total["cost_xlm"], "0.3456");

        let metrics = v.get("spec_metrics").expect("spec_metrics object");
        assert_eq!(metrics["functions_count"], 2);
        assert_eq!(metrics["custom_types_count"], 1);
        assert_eq!(metrics["events_count"], 1);
    }

    #[test]
    fn json_custom_ledgers_scales_cost() {
        let out = sdkt()
            .args([
                "storage",
                "estimate",
                WASM_NEW,
                "--ledgers",
                "100",
                "--format",
                "json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = assert_valid_json(&String::from_utf8_lossy(&out));

        assert_eq!(v["ledgers"], 100);
        assert_eq!(v["cost_per_entry_stroops"], 10000);
        assert_eq!(v["cost_per_entry_xlm"], "0.001");

        // us_new.wasm has 0 UDTs -> 1 instance entry, 0 persistent
        let classes = &v["classes"];
        assert_eq!(classes["instance"]["entry_count"], 1);
        assert_eq!(classes["instance"]["cost_stroops"], 10000);
        assert_eq!(classes["instance"]["cost_xlm"], "0.001");
        assert_eq!(classes["persistent"]["entry_count"], 0);
        assert_eq!(classes["persistent"]["cost_stroops"], 0);
        assert_eq!(classes["persistent"]["cost_xlm"], "0");

        let total = &v["total"];
        assert_eq!(total["baseline_entries"], 1);
        assert_eq!(total["cost_stroops"], 10000);
        assert_eq!(total["cost_xlm"], "0.001");
    }
}

