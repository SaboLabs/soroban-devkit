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

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use predicates::prelude::*;
use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use stellar_xdr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ContractId, ExtensionPoint, Hash, LedgerEntry, LedgerEntryData,
    LedgerEntryExt, Limited, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use tempfile::tempdir;

static WASM_FIXTURE: &[u8] = include_bytes!("fixtures/us_new.wasm");
const CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const WASM_HASH_HEX: &str = "551c5a9c7fd4c4a71e57e9c8a0ece1e7bd506ea065993c63db2dc35516406775";

fn encode_ledger_entry(entry: &LedgerEntry) -> String {
    let mut buf = Vec::new();
    let mut l = Limited::new(&mut buf, Limits::none());
    entry.write_xdr(&mut l).unwrap();
    STANDARD.encode(&buf)
}

fn contract_data_entry_xdr(wasm_hash: [u8; 32]) -> String {
    encode_ledger_entry(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(wasm_hash)),
                storage: None,
            }),
        }),
        ext: LedgerEntryExt::V0,
    })
}

fn contract_code_entry_xdr(wasm_hash: [u8; 32], code: &[u8]) -> String {
    encode_ledger_entry(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(wasm_hash),
            code: code.to_vec().try_into().unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    })
}

fn spawn_mock_rpc_server(wasm_bytes: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let contract_id_hex = "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c";
    let contract_data_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(contract_id_hex.to_string())).unwrap();
    let contract_code_key =
        encode_ledger_key(&LedgerKeyParams::ContractCode(WASM_HASH_HEX.to_string())).unwrap();

    let mut wasm_hash = [0u8; 32];
    hex::decode_to_slice(WASM_HASH_HEX, &mut wasm_hash).unwrap();

    let inspect_xdr = contract_data_entry_xdr(wasm_hash);
    let code_xdr = contract_code_entry_xdr(wasm_hash, wasm_bytes);

    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut sock = match conn {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            let body = if req.contains("\"getLatestLedger\"") {
                r#"{"jsonrpc":"2.0","id":1,"result":{"id":"test","sequence":100,"protocolVersion":20}}"#.to_string()
            } else if req.contains("\"getLedgerEntries\"") {
                if req.contains(&contract_data_key) {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{contract_data_key}","xdr":"{inspect_xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                    )
                } else if req.contains(&contract_code_key) {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{contract_code_key}","xdr":"{code_xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                    )
                } else {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"entries":[],"latestLedger":100}}"#
                        .to_string()
                }
            } else {
                r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                    .to_string()
            };

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });

    url
}

fn sdkt_isolated(cache_dir: &std::path::Path, network_dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.env("SDKT_CACHE_DIR", cache_dir);
    cmd.env("SDKT_NETWORK_DIR", network_dir);
    cmd
}

#[test]
fn test_cli_network_conflicts_rejected() {
    let cache_dir = tempdir().unwrap();
    let network_dir = tempdir().unwrap();

    // 1. wasm metadata: --network with --rpc-url
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "wasm",
            "metadata",
            "--contract",
            CONTRACT_ID,
            "--network",
            "mainnet",
            "--rpc-url",
            "http://127.0.0.1:1234",
        ])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("cannot be used with")
                .or(predicates::str::contains("conflicts with")),
        );

    // 2. wasm metadata: --network with --network-profile
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "wasm",
            "metadata",
            "--contract",
            CONTRACT_ID,
            "--network",
            "testnet",
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("cannot be used with")
                .or(predicates::str::contains("conflicts with")),
        );

    // 3. verify: --network with --rpc-url
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "verify",
            "--contract",
            CONTRACT_ID,
            "--network",
            "mainnet",
            "--rpc-url",
            "http://127.0.0.1:1234",
        ])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("cannot be used with")
                .or(predicates::str::contains("conflicts with")),
        );

    // 4. health: --network with --network-profile
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "health",
            "--contract",
            CONTRACT_ID,
            "--network",
            "futurenet",
            "--network-profile",
            "mocknet",
        ])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("cannot be used with")
                .or(predicates::str::contains("conflicts with")),
        );
}

#[test]
fn test_cli_wasm_metadata_mock_rpc_cache_namespace() {
    let cache_dir = tempdir().unwrap();
    let network_dir = tempdir().unwrap();
    let mock_url = spawn_mock_rpc_server(WASM_FIXTURE);

    // Add a named network profile
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "network",
            "add",
            "profile_alpha",
            "--rpc-url",
            &mock_url,
            "--passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success();

    // 1. First fetch: should be Cache Miss and write to wasm/profile_alpha/
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "wasm",
            "metadata",
            "--contract",
            CONTRACT_ID,
            "--network-profile",
            "profile_alpha",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Cache Status: Miss"))
        .stdout(predicates::str::contains("Network: profile_alpha"));

    let cached_file = cache_dir
        .path()
        .join("wasm")
        .join("profile_alpha")
        .join(format!("{}.json", WASM_HASH_HEX));
    assert!(
        cached_file.exists(),
        "Cache file must exist at {:?}",
        cached_file
    );

    // Verify testnet cache namespace was NOT touched
    let testnet_dir = cache_dir.path().join("wasm").join("testnet");
    assert!(
        !testnet_dir.exists(),
        "testnet cache namespace must not be touched"
    );

    // 2. Second fetch: should be Cache Hit from profile_alpha cache
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "wasm",
            "metadata",
            "--contract",
            CONTRACT_ID,
            "--network-profile",
            "profile_alpha",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Cache Status: Hit"))
        .stdout(predicates::str::contains("Network: profile_alpha"));
}

#[test]
fn test_cli_cache_namespace_isolation_regression() {
    let cache_dir = tempdir().unwrap();
    let network_dir = tempdir().unwrap();
    let mock_url = spawn_mock_rpc_server(WASM_FIXTURE);

    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "network",
            "add",
            "profile_net1",
            "--rpc-url",
            &mock_url,
            "--passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success();

    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "network",
            "add",
            "profile_net2",
            "--rpc-url",
            &mock_url,
            "--passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success();

    // Run wasm metadata on profile_net1
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "wasm",
            "metadata",
            "--contract",
            CONTRACT_ID,
            "--network-profile",
            "profile_net1",
        ])
        .assert()
        .success();

    let net1_file = cache_dir
        .path()
        .join("wasm")
        .join("profile_net1")
        .join(format!("{}.json", WASM_HASH_HEX));
    assert!(net1_file.exists());

    // Plant a fake/poisoned cache entry in testnet
    let testnet_dir = cache_dir.path().join("wasm").join("testnet");
    std::fs::create_dir_all(&testnet_dir).unwrap();
    let testnet_poison_file = testnet_dir.join(format!("{}.json", WASM_HASH_HEX));
    std::fs::write(&testnet_poison_file, b"{\"poisoned\":true}").unwrap();

    // Query on profile_net2: must NOT read from testnet, must write to profile_net2
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "wasm",
            "metadata",
            "--contract",
            CONTRACT_ID,
            "--network-profile",
            "profile_net2",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Cache Status: Miss"));

    let net2_file = cache_dir
        .path()
        .join("wasm")
        .join("profile_net2")
        .join(format!("{}.json", WASM_HASH_HEX));
    assert!(net2_file.exists());

    // The testnet planted file must remain untouched
    let testnet_content = std::fs::read(&testnet_poison_file).unwrap();
    assert_eq!(testnet_content, b"{\"poisoned\":true}");

    // Also assert offline --network mainnet failure does NOT touch testnet
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "wasm",
            "metadata",
            "--contract",
            CONTRACT_ID,
            "--network",
            "mainnet",
        ])
        .assert()
        .failure();

    let testnet_content_after = std::fs::read(&testnet_poison_file).unwrap();
    assert_eq!(testnet_content_after, b"{\"poisoned\":true}");
}

#[test]
fn test_cli_verify_and_health_network_labels_against_mock() {
    let cache_dir = tempdir().unwrap();
    let network_dir = tempdir().unwrap();
    let mock_url = spawn_mock_rpc_server(WASM_FIXTURE);

    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "network",
            "add",
            "custom_endpoint",
            "--rpc-url",
            &mock_url,
            "--passphrase",
            "Test SDF Network ; September 2015",
        ])
        .assert()
        .success();

    // 1. Verify with profile label
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "verify",
            "--contract",
            CONTRACT_ID,
            "--network-profile",
            "custom_endpoint",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Network     : custom_endpoint"));

    // 2. Verify with profile label in JSON format
    let assert_verify_json = sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "verify",
            "--contract",
            CONTRACT_ID,
            "--network-profile",
            "custom_endpoint",
            "--format",
            "json",
        ])
        .assert()
        .success();
    let verify_val: serde_json::Value =
        serde_json::from_slice(&assert_verify_json.get_output().stdout).unwrap();
    assert_eq!(verify_val["network"], "custom_endpoint");

    // 3. Health with profile label
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "health",
            "--contract",
            CONTRACT_ID,
            "--network-profile",
            "custom_endpoint",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Network     : custom_endpoint"));

    // 4. Health with profile label in JSON format
    let assert_health_json = sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "health",
            "--contract",
            CONTRACT_ID,
            "--network-profile",
            "custom_endpoint",
            "--format",
            "json",
        ])
        .assert()
        .success();
    let health_val: serde_json::Value =
        serde_json::from_slice(&assert_health_json.get_output().stdout).unwrap();
    assert_eq!(health_val["network"], "custom_endpoint");

    // 5. Verify & Health with mainnet passphrase resolves to "mainnet" label
    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "verify",
            "--contract",
            CONTRACT_ID,
            "--rpc-url",
            &mock_url,
            "--network-passphrase",
            "Public Global Stellar Network ; September 2015",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Network     : mainnet"));

    sdkt_isolated(cache_dir.path(), network_dir.path())
        .args([
            "health",
            "--contract",
            CONTRACT_ID,
            "--rpc-url",
            &mock_url,
            "--network-passphrase",
            "Public Global Stellar Network ; September 2015",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Network     : mainnet"));
}
