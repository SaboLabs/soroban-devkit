use assert_cmd::Command;

#[test]
fn test_cli_wasm_cache_info_default() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd.arg("wasm").arg("cache").arg("info").assert();
    assert
        .success()
        .stdout(predicates::str::contains("Cache Info for Network"));
}

#[test]
fn test_cli_wasm_cache_info_json() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("cache")
        .arg("info")
        .arg("--format")
        .arg("json")
        .assert();
    assert
        .success()
        .stdout(predicates::str::contains("\"network\":\"testnet\""));
}

#[test]
fn test_cli_wasm_cache_info_json_escaped_quotes() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("cache")
        .arg("info")
        .arg("--network")
        .arg("a\"b")
        .arg("--format")
        .arg("json")
        .assert();

    let output = assert.success().get_output().stdout.clone();
    let stdout_str = std::str::from_utf8(&output).unwrap();

    let json: serde_json::Value = serde_json::from_str(stdout_str).expect("Valid JSON");
    assert_eq!(json["network"], "a\"b");
}

#[test]
fn test_cli_wasm_cache_info_json_escaped_backslash() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("cache")
        .arg("info")
        .arg("--network")
        .arg("a\\b")
        .arg("--format")
        .arg("json")
        .assert();

    let output = assert.success().get_output().stdout.clone();
    let stdout_str = std::str::from_utf8(&output).unwrap();

    let json: serde_json::Value = serde_json::from_str(stdout_str).expect("Valid JSON");
    assert_eq!(json["network"], "a\\b");
}

#[test]
fn test_cli_wasm_cache_info_json_stable_shape() {
    // Clear cache first to ensure 0 values for deterministic output
    let mut clear_cmd = Command::cargo_bin("sdkt").unwrap();
    clear_cmd.arg("wasm").arg("cache").arg("clear").arg("--network").arg("testnet_stable_shape").assert().success();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("cache")
        .arg("info")
        .arg("--network")
        .arg("testnet_stable_shape")
        .arg("--format")
        .arg("json")
        .assert();

    let output = assert.success().get_output().stdout.clone();
    let stdout_str = std::str::from_utf8(&output).unwrap().trim();

    // Verify it is byte-identical to the expected stable JSON shape
    let expected = serde_json::json!({
        "network": "testnet_stable_shape",
        "entry_count": 0,
        "total_metadata_size_bytes": 0,
        "total_wasm_size_bytes": 0
    }).to_string();
    assert_eq!(stdout_str, expected);
}

#[test]
fn test_cli_wasm_cache_clear() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("cache")
        .arg("clear")
        .arg("--network")
        .arg("testnet")
        .assert();
    assert.success().stdout(predicates::str::contains(
        "Cleared all cache entries for testnet.",
    ));
}

#[test]
fn test_cli_wasm_cache_remove() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("cache")
        .arg("remove")
        .arg("fakehash123")
        .assert();
    assert.success().stdout(predicates::str::contains(
        "Removed fakehash123 from testnet cache.",
    ));
}

#[test]
fn test_cli_wasm_inspect_missing_file() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("inspect")
        .arg("non_existent_file.wasm")
        .assert();
    assert
        .failure()
        .stderr(predicates::str::contains("Error reading WASM file"));
}

#[test]
fn test_cli_wasm_inspect_invalid_wasm() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"invalid wasm data").unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd.arg("wasm").arg("inspect").arg(tmp.path()).assert();
    assert
        .failure()
        .stderr(predicates::str::contains("Error parsing WASM metadata"));
}

#[test]
fn test_cli_wasm_inspect_valid_empty_wasm() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    // A minimal valid WASM binary (magic + version 1)
    std::fs::write(tmp.path(), [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd.arg("wasm").arg("inspect").arg(tmp.path()).assert();
    assert
        .success()
        .stdout(predicates::str::contains("WASM Inspection Report"))
        .stdout(predicates::str::contains("Size: 8 bytes"))
        .stdout(predicates::str::contains("Contract Spec Available: No"));
}

#[test]
fn test_cli_wasm_inspect_json() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).unwrap();

    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("inspect")
        .arg(tmp.path())
        .arg("--format")
        .arg("json")
        .assert();
    assert
        .success()
        .stdout(predicates::str::contains("\"size_bytes\": 8"));
}
#[test]
fn test_cli_wasm_metadata_missing_contract() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("metadata")
        // No --contract
        .assert();
    assert.failure();
}
