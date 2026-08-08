//! M41 — `sdkt wasm metadata --contract` behavior tests (hermetic).
//!
//! The on-chain inspection path requires a reachable Soroban RPC. This crate test
//! is hermetic: it asserts the command degrades gracefully (clean non-zero exit,
//! no panic) when no RPC is available, and that the output schema names the
//! enriched fields. The positive "non-empty functions" path is covered by the
//! `sdkt-rpc` fixture test and by the Compatibility CI on-chain step (which uses a
//! committed JSON fixture fallback so it never depends on live testnet).

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    let dir = std::env::temp_dir().join(format!(
        "sdkt-m41-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&dir);
    cmd.env("SDKT_NETWORK_DIR", &dir);
    cmd
}

#[test]
fn wasm_metadata_contract_offline_is_graceful() {
    // Unreachable RPC endpoint: the command must fail cleanly (exit != 0) with an
    // "Error" line, never a Rust panic / unwrap crash.
    sdkt()
        .env("SDKT_NETWORK_DIR", std::env::temp_dir())
        .args([
            "wasm",
            "metadata",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--network",
            "testnet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error"))
        .stderr(predicate::str::contains("panic").not());
}

#[test]
fn wasm_metadata_contract_json_schema_has_abi_field() {
    // Even on the offline failure path, the inspection report type carries the
    // M41-enriched fields. We validate the (de)serialization contract here using
    // the same struct the CLI prints, proving the schema is stable.
    use sdkt_rpc::inspect::ContractInspection;
    let json = r#"{
        "contract_id": "CABC",
        "wasm_hash": "deadbeef",
        "wasm_size": 2048,
        "abi": {"functions": ["mint"], "events": ["transfer"], "types": ["Asset"]},
        "storage_summary": {"instance_entries":1,"persistent_entries":0,"temporary_entries":0},
        "ttl_info": null,
        "storage_keys": []
    }"#;
    let parsed: ContractInspection =
        serde_json::from_str(json).expect("enriched schema deserializes");
    assert_eq!(parsed.wasm_size, Some(2048));
    assert_eq!(parsed.abi.unwrap().functions, vec!["mint".to_string()]);
}
