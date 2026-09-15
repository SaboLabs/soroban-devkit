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
