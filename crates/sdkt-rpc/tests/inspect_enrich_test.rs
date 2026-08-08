//! M41 — on-chain inspection enrichment tests.
//!
//! `inspect_contract` performs live RPC calls, which this crate has no mock for
//! (consistent with the existing `inspect.rs`/`wasm.rs` tests). We therefore
//! validate the enrichment *logic* against a real Soroban contract WASM fixture:
//! the exact `parse_contract_spec` -> `ContractAbiSummary` chain that
//! `inspect_contract` runs on the on-chain bytecode. We also assert the
//! graceful-default serialization (missing ABI -> `null`) matches the documented
//! contract.

use sdkt_rpc::inspect::{ContractAbiSummary, ContractInspection};
use sdkt_wasm::parse_contract_spec;

#[test]
fn abi_summary_from_real_fixture_lists_symbols() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../sdkt-cli/tests/fixtures/us_new.wasm"
    ))
    .expect("fixture WASM present");

    let spec = parse_contract_spec(&bytes).expect("fixture parses as a contractspecv0 WASM");
    let summary = ContractAbiSummary::from_spec(&spec);

    // A real Soroban contract must declare at least one function.
    assert!(
        !summary.functions.is_empty(),
        "expected non-empty function list from real fixture"
    );
    // Print for visibility in `--nocapture`; not asserted on exact names.
    eprintln!(
        "fixture ABI: {} functions, {} events, {} types",
        summary.functions.len(),
        summary.events.len(),
        summary.types.len()
    );
}

#[test]
fn inspection_without_abi_serializes_gracefully() {
    // Mirror the degraded state inspect_contract returns when on-chain code is
    // unavailable: id + hash recovered, abi/size left None/empty.
    let inspection = ContractInspection {
        contract_id: "C123".to_string(),
        wasm_hash: "abcd".to_string(),
        wasm_size: None,
        abi: None,
        storage_summary: Default::default(),
        ttl_info: None,
        storage_keys: vec![],
    };
    let json = serde_json::to_string(&inspection).unwrap();
    assert!(
        json.contains("\"abi\":null"),
        "missing ABI must serialize as null"
    );
    assert!(
        json.contains("\"wasm_size\":null"),
        "missing size must serialize as null"
    );
}

#[test]
fn inspection_with_abi_serializes_names() {
    let inspection = ContractInspection {
        contract_id: "C123".to_string(),
        wasm_hash: "abcd".to_string(),
        wasm_size: Some(1024),
        abi: Some(ContractAbiSummary {
            functions: vec!["mint".into(), "burn".into()],
            events: vec!["transfer".into()],
            types: vec!["Asset".into()],
        }),
        storage_summary: Default::default(),
        ttl_info: None,
        storage_keys: vec![],
    };
    let json = serde_json::to_string(&inspection).unwrap();
    assert!(json.contains("\"functions\":[\"mint\",\"burn\"]"));
    assert!(json.contains("\"wasm_size\":1024"));
}
