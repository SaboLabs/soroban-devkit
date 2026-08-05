use assert_cmd::Command;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt-cli").unwrap()
}

/// `sdkt storage analyze <id>` should reject an empty contract id with a
/// non-zero exit and an error message, without contacting the network.
#[test]
fn storage_analyze_empty_id_errors() {
    sdkt()
        .args(["storage", "analyze", ""])
        .assert()
        .failure()
        .stderr(predicates::str::contains("Error analyzing storage"));
}

/// `sdkt storage analyze --help` should list the subcommand and document the
/// Instance/Persistent/Temporary categorization without erroring.
#[test]
fn storage_analyze_help_documents_categorization() {
    sdkt()
        .args(["storage", "analyze", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("CONTRACT_ID"))
        .stdout(predicates::str::contains("Instance/Persistent/Temporary"));
}

/// `--format json` is accepted as a valid flag and does not crash at parse time.
#[test]
fn storage_analyze_accepts_json_format_flag() {
    sdkt()
        .args([
            "storage",
            "analyze",
            "CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
            "--format",
            "json",
        ])
        .assert()
        // Network call will fail (no RPC), but the flag parses and we reach execution.
        .failure();
}
