//! Tests for the browser glue crate.
//!
//! These run on the **native** target: `inspect_wasm` itself is
//! `wasm-bindgen`-gated, so the tests exercise the exact same code path it
//! wraps (`sdkt_wasm::parse_metadata` + `parse_contract_spec`) plus this
//! crate's user-facing error mapping, which is the part that is genuinely new
//! here. Run with:
//!
//! ```text
//! cargo test --manifest-path crates/sdkt-playground/Cargo.toml
//! ```

use sdkt_playground::{inspect_parts, user_message_for_test as user_message};
use sdkt_wasm::WasmError;

const US_OLD: &[u8] = include_bytes!("../../sdkt-cli/tests/fixtures/us_old.wasm");
const US_NEW: &[u8] = include_bytes!("../../sdkt-cli/tests/fixtures/us_new.wasm");

/// Minimal valid module: magic + version, no sections.
const BARE_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

#[test]
fn valid_fixture_yields_metadata_and_spec() {
    let r = inspect_parts(US_OLD).expect("fixture must inspect");
    let (meta, spec, spec_err) = (r.metadata, r.spec, r.spec_error);
    assert_eq!(meta.size_bytes, US_OLD.len());
    assert_eq!(meta.hash.len(), 64, "sha-256 hex digest");
    assert_eq!(meta.version, 1);
    assert!(
        meta.custom_sections.iter().any(|s| s == "contractspecv0"),
        "fixture carries a contract spec section: {:?}",
        meta.custom_sections
    );
    let spec = spec.expect("fixture has a contractspecv0 section");
    assert!(spec_err.is_none());
    assert_eq!(spec.functions.len(), 2, "us_old declares transfer + mint");
    let names: Vec<&str> = spec.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"transfer"), "functions: {:?}", names);
    assert!(names.contains(&"mint"), "functions: {:?}", names);
    assert_eq!(spec.events.len(), 1);
    assert_eq!(spec.custom_types.len(), 1);
}

#[test]
fn second_fixture_differs_from_first() {
    let a = inspect_parts(US_OLD).unwrap();
    let b = inspect_parts(US_NEW).unwrap();
    let (m_old, s_old) = (a.metadata, a.spec);
    let (m_new, s_new) = (b.metadata, b.spec);
    assert_ne!(m_old.hash, m_new.hash, "distinct binaries hash differently");
    let old_fns = s_old.unwrap().functions.len();
    let new_fns = s_new.unwrap().functions.len();
    assert!(
        new_fns >= old_fns,
        "us_new adds balance(): {} -> {}",
        old_fns,
        new_fns
    );
}

#[test]
fn empty_input_is_rejected_with_friendly_message() {
    let err = inspect_parts(&[]).expect_err("empty input must fail");
    let msg = user_message(&err);
    assert!(matches!(err, WasmError::Empty));
    assert!(msg.contains("empty"), "msg: {msg}");
    assert!(!msg.contains("panicked"), "no panic text leaks: {msg}");
}

#[test]
fn non_wasm_bytes_are_rejected() {
    // Plain text — no WASM magic number.
    let err =
        inspect_parts(b"this is definitely not webassembly").expect_err("non-wasm input must fail");
    let msg = user_message(&err);
    assert!(matches!(err, WasmError::Parse(_)));
    assert!(msg.contains("not a valid WebAssembly module"), "msg: {msg}");
}

#[test]
fn malformed_wasm_is_rejected() {
    // Correct magic + version, then a truncated/garbage section header.
    let mut bytes = BARE_WASM.to_vec();
    bytes.extend_from_slice(&[0x07, 0xff, 0xff, 0xff]);
    let err = inspect_parts(&bytes).expect_err("malformed module must fail");
    let msg = user_message(&err);
    assert!(matches!(err, WasmError::Parse(_)));
    assert!(!msg.is_empty());
    assert!(
        !msg.contains("BinaryReaderError"),
        "internal type leaked: {msg}"
    );
}

#[test]
fn valid_module_without_contract_spec_still_inspects() {
    // A bare module is valid WebAssembly but is not a Soroban contract: metadata
    // must succeed and the spec must be reported as absent (mirrors the CLI's
    // "Contract Spec Available: No").
    let r = inspect_parts(BARE_WASM).expect("bare module is valid wasm");
    let (meta, spec, spec_err) = (r.metadata, r.spec, r.spec_error);
    assert_eq!(meta.size_bytes, 8);
    assert_eq!(meta.version, 1);
    assert!(meta.exports.is_empty());
    assert!(spec.is_none(), "no contract spec expected");
    let msg = spec_err.expect("an informational spec message is provided");
    assert!(msg.contains("contractspecv0"), "msg: {msg}");
}

#[test]
fn repeated_inspection_is_deterministic() {
    let a = inspect_parts(US_OLD).unwrap().metadata;
    let b = inspect_parts(US_OLD).unwrap().metadata;
    assert_eq!(a.hash, b.hash);
    assert_eq!(a.size_bytes, b.size_bytes);
    assert_eq!(a.custom_sections, b.custom_sections);
}

#[test]
fn larger_input_does_not_break_parsing() {
    // Valid module followed by a large custom section (id 0) carrying padding.
    // Exercises the "reasonably large WASM" path without needing a real big
    // contract in the repo.
    let name = b"padding";
    let payload_len = 64 * 1024;
    let mut section = Vec::new();
    section.push(name.len() as u8);
    section.extend_from_slice(name);
    section.extend(std::iter::repeat_n(0x00, payload_len));

    let mut bytes = BARE_WASM.to_vec();
    bytes.push(0x00); // custom section id
                      // LEB128 length
    let mut len = section.len();
    loop {
        let mut byte = (len & 0x7f) as u8;
        len >>= 7;
        if len != 0 {
            byte |= 0x80;
        }
        bytes.push(byte);
        if len == 0 {
            break;
        }
    }
    bytes.extend_from_slice(&section);

    let meta = inspect_parts(&bytes).expect("large module parses").metadata;
    assert_eq!(meta.size_bytes, bytes.len());
    assert!(meta.custom_sections.iter().any(|s| s == "padding"));
}
