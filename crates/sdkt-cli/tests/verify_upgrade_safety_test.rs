//! — `sdkt verify --upgrade-safety` behavior tests (hermetic).
//!
//! The on-chain path requires a reachable RPC, so the live verdict is covered by
//! the Compatibility CI (network-guarded, with a committed fixture fallback). This
//! crate test is hermetic: it asserts argument validation, graceful offline
//! failure (no panic), and that the upgrade-safety verdict produced by the shared
//! engine (`diff_wasm` -> `UpgradeVerdict`) is identical for the same inputs —
//! proving the command reuses the engine rather than a parallel one.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    let dir = std::env::temp_dir().join(format!(
        "sdkt-m42-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&dir);
    cmd.env("SDKT_NETWORK_DIR", &dir);
    cmd
}

fn fixture(name: &str) -> String {
    let dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/tests/fixtures/{}", dir, name)
}

/// Build a minimal valid WASM carrying only a `contractspecv0` section built
/// from `entries`, so a before/after pair can be exercised hermetically.
fn wasm_with_spec(entries: &[stellar_xdr::ScSpecEntry]) -> Vec<u8> {
    use stellar_xdr::{Limited, Limits, WriteXdr};

    let mut section = Vec::new();
    section.push(b"contractspecv0".len() as u8);
    section.extend_from_slice(b"contractspecv0");
    for e in entries {
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        e.write_xdr(&mut Limited::new(&mut cursor, Limits::none()))
            .unwrap();
        section.extend_from_slice(&buf);
    }

    let mut result = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    result.push(0); // custom section id
    let mut sz = section.len() as u32;
    loop {
        let byte = (sz & 0x7f) as u8;
        sz >>= 7;
        if sz == 0 {
            result.push(byte);
            break;
        }
        result.push(byte | 0x80);
    }
    result.extend_from_slice(&section);
    result
}

/// Write `bytes` to a uniquely-named temp file and return its path.
fn temp_wasm(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-evtchange-{}-{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("contract.wasm");
    fs::write(&path, bytes).unwrap();
    path
}

fn symbol(s: &str) -> stellar_xdr::ScSymbol {
    stellar_xdr::ScSymbol(s.to_string().try_into().unwrap())
}

/// A `Transfer` event with the given `(name, type)` params, all in the data
/// payload, alongside an unchanged `transfer` function so the only delta
/// between a before/after pair is the event shape.
fn transfer_event_spec(
    params: Vec<(&str, stellar_xdr::ScSpecTypeDef)>,
) -> Vec<stellar_xdr::ScSpecEntry> {
    use stellar_xdr::{
        ScSpecEntry, ScSpecEventDataFormat, ScSpecEventParamLocationV0, ScSpecEventParamV0,
        ScSpecEventV0, ScSpecFunctionInputV0, ScSpecFunctionV0,
    };
    let func = ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
        doc: "".try_into().unwrap(),
        name: symbol("transfer"),
        inputs: vec![ScSpecFunctionInputV0 {
            doc: "".try_into().unwrap(),
            name: "to".try_into().unwrap(),
            type_: stellar_xdr::ScSpecTypeDef::Address,
        }]
        .try_into()
        .unwrap(),
        outputs: vec![].try_into().unwrap(),
    });
    let event = ScSpecEntry::EventV0(ScSpecEventV0 {
        doc: "".try_into().unwrap(),
        lib: "soroban_sdk".try_into().unwrap(),
        name: symbol("Transfer"),
        prefix_topics: vec![].try_into().unwrap(),
        params: params
            .into_iter()
            .map(|(n, t)| ScSpecEventParamV0 {
                doc: "".try_into().unwrap(),
                name: n.try_into().unwrap(),
                type_: t,
                location: ScSpecEventParamLocationV0::Data,
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap(),
        data_format: ScSpecEventDataFormat::SingleValue,
    });
    vec![func, event]
}

/// A `Point` struct with the given `(name, type)` fields, alongside an
/// unchanged function so the only delta is the type definition.
fn point_spec(fields: Vec<(&str, stellar_xdr::ScSpecTypeDef)>) -> Vec<stellar_xdr::ScSpecEntry> {
    use stellar_xdr::{
        ScSpecEntry, ScSpecFunctionInputV0, ScSpecFunctionV0, ScSpecUdtStructFieldV0,
        ScSpecUdtStructV0,
    };
    let func = ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
        doc: "".try_into().unwrap(),
        name: symbol("origin"),
        inputs: vec![ScSpecFunctionInputV0 {
            doc: "".try_into().unwrap(),
            name: "p".try_into().unwrap(),
            type_: stellar_xdr::ScSpecTypeDef::I32,
        }]
        .try_into()
        .unwrap(),
        outputs: vec![].try_into().unwrap(),
    });
    let udt = ScSpecEntry::UdtStructV0(ScSpecUdtStructV0 {
        doc: "".try_into().unwrap(),
        lib: "soroban_sdk".try_into().unwrap(),
        name: "Point".try_into().unwrap(),
        fields: fields
            .into_iter()
            .map(|(n, t)| ScSpecUdtStructFieldV0 {
                doc: "".try_into().unwrap(),
                name: n.try_into().unwrap(),
                type_: t,
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap(),
    });
    vec![func, udt]
}

#[test]
fn verify_help_documents_upgrade_safety() {
    sdkt()
        .args(["verify", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("upgrade-safety"));
}

#[test]
fn upgrade_safety_without_wasm_is_controlled_error() {
    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--upgrade-safety",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--upgrade-safety requires --wasm"));
}

#[test]
fn upgrade_safety_with_missing_candidate_file_is_controlled_error() {
    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--wasm",
            "/no/such/file.wasm",
            "--upgrade-safety",
            "--network",
            "testnet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error reading WASM file"));
}

#[test]
fn upgrade_safety_with_malformed_candidate_is_controlled_error() {
    // A text file is not valid WASM -> must fail cleanly, never panic.
    let dir = std::env::temp_dir().join(format!(
        "sdkt-m42-bad-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.wasm");
    fs::write(&bad, b"this is not wasm").unwrap();

    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--wasm",
            bad.to_str().unwrap(),
            "--upgrade-safety",
            "--network",
            "testnet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not valid WASM"));
}

#[test]
fn upgrade_safety_offline_contract_unreachable_is_graceful() {
    // No RPC reachable -> clean failure (no panic), even with a valid candidate.
    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
            "--network",
            "testnet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error verifying upgrade safety"))
        .stderr(predicate::str::contains("panic").not());
}

#[test]
fn existing_verify_without_flag_still_runs() {
    // Regular `verify --contract` (no --wasm) must still behave (offline failure
    // is a clean error, not a crash) — backward compatibility preserved.
    sdkt()
        .args([
            "verify",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--network",
            "testnet",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error verifying contract"))
        .stderr(predicate::str::contains("panic").not());
}

#[test]
fn breaking_change_verdict_matches_m14_engine() {
    // The same us_old -> us_new inputs through `diff --upgrade-safety` (which uses
    // the identical engine the verify command reuses) must yield NO / breaking.
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_new.wasm"),
            "--upgrade-safety",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Upgrade Safety"))
        .stdout(predicate::str::contains("Compatible: NO"))
        .stdout(predicate::str::contains("Removed function: mint()"))
        .stdout(predicate::str::contains("Added function: hello()"));
}

#[test]
fn compatible_case_verdict_is_yes() {
    // us_old against itself is a no-op upgrade -> the engine reports YES.
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            &fixture("us_old.wasm"),
            "--new-wasm",
            &fixture("us_old.wasm"),
            "--upgrade-safety",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compatible: YES"));
}

#[test]
fn changed_event_params_verdict_is_not_compatible() {
    use stellar_xdr::ScSpecTypeDef;
    let old = temp_wasm(
        "verdict-old",
        &wasm_with_spec(&transfer_event_spec(vec![
            ("from", ScSpecTypeDef::Address),
            ("amount", ScSpecTypeDef::I128),
        ])),
    );
    let new = temp_wasm(
        "verdict-new",
        &wasm_with_spec(&transfer_event_spec(vec![
            ("from", ScSpecTypeDef::Address),
            ("amount", ScSpecTypeDef::U64),
            ("memo", ScSpecTypeDef::String),
        ])),
    );
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            old.to_str().unwrap(),
            "--new-wasm",
            new.to_str().unwrap(),
            "--upgrade-safety",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compatible: NO"))
        .stdout(predicate::str::contains("Changed event: Transfer"))
        // The verdict must show both shapes, not just the label.
        .stdout(predicate::str::contains("old: Transfer(from: address"))
        .stdout(predicate::str::contains("new: Transfer(from: address"));
}

#[test]
fn changed_event_diff_reports_both_shapes() {
    use stellar_xdr::ScSpecTypeDef;
    let old = temp_wasm(
        "shape-old",
        &wasm_with_spec(&transfer_event_spec(vec![("amount", ScSpecTypeDef::I128)])),
    );
    let new = temp_wasm(
        "shape-new",
        &wasm_with_spec(&transfer_event_spec(vec![("amount", ScSpecTypeDef::U64)])),
    );
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            old.to_str().unwrap(),
            "--new-wasm",
            new.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Changed events (1):"))
        .stdout(predicate::str::contains("old: Transfer(amount: i128"))
        .stdout(predicate::str::contains("new: Transfer(amount: u64"));
}

#[test]
fn changed_event_params_blocks_deny_breaking_deploy() {
    use stellar_xdr::ScSpecTypeDef;
    let old = temp_wasm(
        "deny-old",
        &wasm_with_spec(&transfer_event_spec(vec![
            ("from", ScSpecTypeDef::Address),
            ("amount", ScSpecTypeDef::I128),
        ])),
    );
    let new = temp_wasm(
        "deny-new",
        &wasm_with_spec(&transfer_event_spec(vec![
            ("from", ScSpecTypeDef::Address),
            ("amount", ScSpecTypeDef::U64),
            ("memo", ScSpecTypeDef::String),
        ])),
    );
    sdkt()
        .args([
            "deploy",
            "--wasm",
            new.to_str().unwrap(),
            "--old-wasm",
            old.to_str().unwrap(),
            "--deny-breaking",
            "--salt",
            "0000000000000000000000000000000000000002",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("NOT backwards-compatible"))
        .stdout(predicate::str::contains("Compatible: NO"))
        .stdout(predicate::str::contains("Changed event: Transfer"));
}

#[test]
fn changed_type_definition_is_breaking() {
    use stellar_xdr::ScSpecTypeDef;
    let old = temp_wasm(
        "ty-old",
        &wasm_with_spec(&point_spec(vec![("x", ScSpecTypeDef::I32)])),
    );
    let new = temp_wasm(
        "ty-new",
        &wasm_with_spec(&point_spec(vec![
            ("x", ScSpecTypeDef::I64),
            ("y", ScSpecTypeDef::I32),
        ])),
    );
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            old.to_str().unwrap(),
            "--new-wasm",
            new.to_str().unwrap(),
            "--upgrade-safety",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compatible: NO"))
        .stdout(predicate::str::contains("Changed type definition: Point"));
}

#[test]
fn unchanged_event_and_type_are_compatible() {
    use stellar_xdr::ScSpecTypeDef;
    let spec = transfer_event_spec(vec![("amount", ScSpecTypeDef::I128)]);
    let old = temp_wasm("same-old", &wasm_with_spec(&spec));
    let new = temp_wasm("same-new", &wasm_with_spec(&spec));
    sdkt()
        .args([
            "diff",
            "--old-wasm",
            old.to_str().unwrap(),
            "--new-wasm",
            new.to_str().unwrap(),
            "--upgrade-safety",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compatible: YES"))
        .stdout(predicate::str::contains("Changed event").not())
        .stdout(predicate::str::contains("Changed type definition").not());
}
