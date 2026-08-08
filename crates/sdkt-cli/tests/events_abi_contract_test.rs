//! M43 — on-chain ABI event decoding tests (hermetic).
//!
//! The `sdkt events --abi-contract <id>` path fetches a deployed contract's
//! on-chain WASM (M41) and parses it to a `ContractSpec`, which is then passed to
//! the M10 `decode_event_topics` engine. This crate test proves the deterministic
//! part of that flow WITHOUT a network: it builds the same `ContractSpec` shape the
//! command would feed the decoder and asserts the decoded event label/fields, using
//! a real Soroban WASM fixture's parsed spec where possible.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    let dir = std::env::temp_dir().join(format!(
        "sdkt-m43-{}",
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
fn events_help_documents_abi_contract() {
    sdkt()
        .args(["events", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("abi-contract"));
}

#[test]
fn abi_and_abi_contract_are_mutually_exclusive() {
    sdkt()
        .args([
            "events",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--abi",
            "/no/such.wasm",
            "--abi-contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "specify only one of --abi or --abi-contract",
        ));
}

#[test]
fn abi_contract_offline_is_graceful() {
    // No RPC reachable -> clean failure (no panic), actionable error message.
    sdkt()
        .args([
            "events",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--abi-contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error"))
        .stderr(predicate::str::contains("panic").not());
}

#[test]
fn existing_abi_local_path_still_resolves() {
    // `--abi <missing file>` must still hit the local-ABI branch and fail with a
    // controlled "Failed to read WASM" error (not a panic), proving the existing
    // behavior is preserved.
    sdkt()
        .args([
            "events",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--abi",
            "/no/such/file.wasm",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Failed to read WASM"));
}

/// Deterministically proves the M43 data flow: a `ContractSpec` (the exact shape
/// produced by `parse_contract_spec` on a deployed WASM) fed to the M10
/// `decode_event_topics` engine yields the event's decoded label — the same call
/// the live `--abi-contract` path makes internally. Uses a hand-built spec so the
/// assertion is independent of any fixture's emitted events.
#[test]
fn deployed_spec_decodes_event_label() {
    use sdkt_wasm::spec::{ContractEvent, ContractSpec};

    let spec = ContractSpec {
        env_meta: None,
        functions: vec![],
        custom_types: vec![],
        events: vec![ContractEvent {
            name: "Mint".to_string(),
            doc: "Mint event".to_string(),
        }],
    };

    // The decoder's first topic is the event symbol (ScVal::Symbol). Event-based
    // labeling is applied to the data values (hinted with the event name), so we
    // pass a data ScVal alongside the topic to exercise the labeled decode path.
    let topic = stellar_xdr::ScVal::Symbol(stellar_xdr::ScSymbol("Mint".try_into().unwrap()));
    let data = vec![stellar_xdr::ScVal::U64(100)];
    let decoded = sdkt_xdr::abi_decode::decode_event_topics(&spec, &[topic], &data);

    // The data value must be labeled with the matched event name ("event[Mint]").
    assert!(
        decoded.iter().any(|d| d.label.contains("event[Mint]")),
        "expected an 'event[Mint]' labeled decode from the deployed-spec decode path; got: {:?}",
        decoded.iter().map(|d| d.label.clone()).collect::<Vec<_>>()
    );
}
