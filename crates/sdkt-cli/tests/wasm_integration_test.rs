use assert_cmd::Command;

#[test]
fn test_cli_wasm_cache_info_default() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let assert = cmd.arg("wasm").arg("cache").arg("info").assert();
    assert
        .success()
        .stdout(predicates::str::contains("Cache Info for Network"));
}

#[test]
fn test_cli_wasm_cache_info_json() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
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
fn test_cli_wasm_cache_clear() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
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
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
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
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
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

    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
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

    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
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

    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
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
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("metadata")
        // No --contract
        .assert();
    assert.failure();
}
