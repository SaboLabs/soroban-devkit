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
fn test_cli_wasm_metadata_missing_contract() {
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let assert = cmd
        .arg("wasm")
        .arg("metadata")
        // No --contract
        .assert();
    assert.failure();
}
