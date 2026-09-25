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
        .stderr(predicates::str::contains("source parse error"));
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

// ---------------------------------------------------------------------------
// Directory audits (#178)
// ---------------------------------------------------------------------------

fn write_file(dir: &TempDir, rel: &str, content: &str) -> std::path::PathBuf {
    let p = dir.path().join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    p
}

/// Single-file fast path must keep working: same header, same findings.
#[test]
fn audit_single_file_regression() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "bad.rs", "pub fn mint_token(to: Address) { }\n");
    sdkt()
        .args(["audit", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::starts_with("Static Analysis Report: "))
        .stdout(predicates::str::contains("AUTH-001"))
        .stdout(predicates::str::contains(
            "Severity: 1 critical, 0 warning, 0 info (1 total)",
        ));
}

/// Directory with 2+ files: every file is audited and findings keep the
/// originating file path.
#[test]
fn audit_directory_two_files_attribution() {
    let dir = TempDir::new().unwrap();
    write_file(
        &dir,
        "a_bad.rs",
        "pub fn transfer(from: Address, to: Address, amount: i128) { }\n",
    );
    write_file(
        &dir,
        "b_ok.rs",
        "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n",
    );
    sdkt()
        .args(["audit", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "Static Analysis Report: {}",
            dir.path().join("a_bad.rs").display()
        )))
        .stdout(predicates::str::contains(format!(
            "Static Analysis Report: {}",
            dir.path().join("b_ok.rs").display()
        )))
        .stdout(predicates::str::contains("AUTH-004"))
        .stdout(predicates::str::contains(
            "Audited 2 files (1 with findings)",
        ));
}

/// Nested directories are walked recursively and the per-file order is the
/// sorted path order (independent of readdir order).
#[test]
fn audit_directory_nested_deterministic_order() {
    let dir = TempDir::new().unwrap();
    // Create in reverse order so readdir order would differ from sorted order.
    write_file(&dir, "z_dir/m.rs", "pub fn zfn() { }\n");
    write_file(&dir, "a_dir/b.rs", "pub fn afn() { }\n");
    let out = sdkt()
        .args(["audit", dir.path().to_str().unwrap(), "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let files: Vec<String> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["file"].as_str().unwrap().to_string())
        .collect();
    let a = dir.path().join("a_dir").join("b.rs");
    let z = dir.path().join("z_dir").join("m.rs");
    assert_eq!(
        files,
        vec![a.display().to_string(), z.display().to_string()],
        "per-file order must be sorted path order"
    );
}

/// One unparseable file yields a PARSE-001 diagnostic finding and does not
/// abort the remaining files.
#[test]
fn audit_directory_unparseable_file_continues() {
    let dir = TempDir::new().unwrap();
    write_file(&dir, "bad_syntax.rs", "fn { not rust code ");
    write_file(
        &dir,
        "good.rs",
        "pub fn transfer(from: Address, to: Address, amount: i128) { }\n",
    );
    sdkt()
        .args(["audit", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("PARSE-001"))
        .stdout(predicates::str::contains("AUTH-004"))
        .stdout(predicates::str::contains(
            "Audited 2 files (2 with findings)",
        ));
}

/// An unreadable file yields an IO-001 diagnostic finding and does not abort.
#[cfg(unix)]
#[test]
fn audit_directory_unreadable_file_continues() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let locked = write_file(&dir, "locked.rs", "pub fn x() { }\n");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    write_file(
        &dir,
        "good.rs",
        "pub fn transfer(from: Address, to: Address, amount: i128) { }\n",
    );
    let result = sdkt()
        .args(["audit", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("IO-001"))
        .stdout(predicates::str::contains("AUTH-004"))
        .get_output()
        .stdout
        .clone();
    // Restore permissions so TempDir cleanup can remove the file.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
    let stdout = String::from_utf8(result).unwrap();
    assert!(stdout.contains("2 files"));
}

/// JSON shape for multi-file audits: additive envelope { files: [{file,
/// report}], totals }, each finding carrying its originating `file` path.
#[test]
fn audit_directory_json_envelope() {
    let dir = TempDir::new().unwrap();
    write_file(&dir, "bad.rs", "pub fn initialize(admin: Address) { }\n");
    write_file(
        &dir,
        "ok.rs",
        "pub fn balance_of(who: Address) -> u32 { require_auth(); 0 }\n",
    );
    let out = sdkt()
        .args(["audit", dir.path().to_str().unwrap(), "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    let files = v["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|f| f["file"].is_string()));
    assert!(files.iter().all(|f| f["report"]["findings"].is_array()));
    assert!(files
        .iter()
        .all(|f| f["report"]["summary"]["total"].is_u64()));

    // Findings carry the originating file path.
    for f in files {
        for finding in f["report"]["findings"].as_array().unwrap() {
            assert_eq!(finding["file"], f["file"]);
        }
    }

    let totals = &v["totals"];
    assert_eq!(totals["files"], 2);
    assert_eq!(totals["files_with_findings"], 1);
    assert_eq!(
        totals["total"],
        totals["critical"].as_u64().unwrap()
            + totals["warning"].as_u64().unwrap()
            + totals["info"].as_u64().unwrap()
    );
}

/// --disable applies to every file in a directory audit.
#[test]
fn audit_directory_disable_rule_applies_to_all_files() {
    let dir = TempDir::new().unwrap();
    write_file(&dir, "a.rs", "pub fn initialize(admin: Address) { }\n");
    write_file(
        &dir,
        "b.rs",
        "pub fn transfer(from: Address, to: Address, amount: i128) { }\n",
    );
    sdkt()
        .args([
            "audit",
            dir.path().to_str().unwrap(),
            "--disable",
            "AUTH-003",
            "--disable",
            "AUTH-004",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Severity: 0 critical, 0 warning, 0 info (0 total)",
        ));
}

/// Empty directory (no *.rs) is an error.
#[test]
fn audit_directory_without_rust_files_errors() {
    let dir = TempDir::new().unwrap();
    sdkt()
        .args(["audit", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "no Rust source files found under",
        ));
}

/// `--disable PARSE-001` silences the synthetic parse diagnostic too, and the
/// other files in the tree are still audited.
#[test]
fn audit_directory_disable_parse001_suppresses_diagnostic() {
    let dir = TempDir::new().unwrap();
    write_file(&dir, "broken.rs", "this is not rust @@@ {{{\n");
    write_file(&dir, "ok.rs", "pub fn heal() -> u64 { 1 }\n");
    sdkt()
        .args([
            "audit",
            dir.path().to_str().unwrap(),
            "--disable",
            "PARSE-001",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("PARSE-001").not())
        .stdout(predicates::str::contains("Audited 2 files"));
}
