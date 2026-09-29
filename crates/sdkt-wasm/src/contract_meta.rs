//! Contract metadata (`contractmetav0`) parser for Soroban contracts.
//!
//! Every Soroban contract WASM may carry a `contractmetav0` custom section
//! holding author-declared key/value provenance entries written by the SDK
//! (`rsver`, `rssdkver`, `cliver`, workspace metadata, ...). The payload is a
//! sequence of concatenated XDR [`stellar_xdr::ScMetaEntry`] values.
//!
//! This mirrors the `contractspecv0` handling in [`crate::spec`]: the XDR is
//! streamed through [`stellar_xdr::Limited`] and mapped into a small,
//! project-owned serde model ([`ContractMetaEntry`]) so the CLI, the cache and
//! the Web Playground can all render the same human-readable values.

use serde::{Deserialize, Serialize};
use std::io::Cursor;
use stellar_xdr::{Limited, Limits, ReadXdr, ScMetaEntry};
use wasmparser::Payload;

use crate::WasmError;

/// Name of the custom section that carries author-declared contract metadata.
pub const CONTRACT_META_V0: &str = "contractmetav0";

/// A single decoded `contractmetav0` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractMetaEntry {
    /// Metadata key (e.g. `rsver`, `rssdkver`, `cliver`).
    pub key: String,
    /// Human-readable metadata value.
    pub value: String,
}

/// Decodes a `contractmetav0` section payload into typed entries.
///
/// The payload is a sequence of concatenated XDR `ScMetaEntry` values. An
/// empty payload yields an empty list. A malformed payload yields
/// [`WasmError::MetaXdr`] rather than panicking.
pub fn decode_contract_meta_section(data: &[u8]) -> Result<Vec<ContractMetaEntry>, WasmError> {
    let mut entries = Vec::new();
    let mut items = data;
    // A metadata section is a set of concatenated `ScMetaEntry` values;
    // `ScMetaEntry::read_xdr` consumes one at a time, so loop until the buffer
    // is exhausted (or an XDR error is reached).
    while !items.is_empty() {
        let mut cursor = Cursor::new(items);
        let mut limited = Limited::new(&mut cursor, Limits::none());
        let entry = ScMetaEntry::read_xdr(&mut limited).map_err(WasmError::MetaXdr)?;
        let consumed = cursor.position() as usize;
        items = &items[consumed..];

        match entry {
            ScMetaEntry::ScMetaV0(v0) => entries.push(ContractMetaEntry {
                key: v0.key.to_utf8_string_lossy(),
                value: v0.val.to_utf8_string_lossy(),
            }),
        }
    }
    Ok(entries)
}

/// Parses all `contractmetav0` entries from compiled WASM bytes.
///
/// Multiple `contractmetav0` sections (which can appear when several crates in
/// a build each contribute metadata) are concatenated in file order. A WASM
/// without the section yields an empty list; empty input yields
/// [`WasmError::Empty`].
pub fn parse_contract_meta(raw_wasm: &[u8]) -> Result<Vec<ContractMetaEntry>, WasmError> {
    if raw_wasm.is_empty() {
        return Err(WasmError::Empty);
    }

    let parser = wasmparser::Parser::new(0);
    let mut entries = Vec::new();
    for payload in parser.parse_all(raw_wasm) {
        if let Payload::CustomSection(reader) = payload? {
            if reader.name() == CONTRACT_META_V0 {
                entries.extend(decode_contract_meta_section(reader.data())?);
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real contract fixture that carries `contractmetav0` metadata.
    const US_NEW_WASM: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../sdkt-cli/tests/fixtures/us_new.wasm"
    ));

    /// Real contract fixture without a `contractmetav0` section.
    const US_OLD_WASM: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../sdkt-cli/tests/fixtures/us_old.wasm"
    ));

    /// Assemble a minimal valid WASM carrying one custom section with the
    /// given name and raw payload.
    fn custom_section_wasm(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut section = Vec::new();
        section.push(name.len() as u8);
        section.extend_from_slice(name.as_bytes());
        section.extend_from_slice(payload);

        let mut result = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        result.push(0); // custom section id
        let mut sz = section.len() as u32;
        let mut size_bytes = Vec::new();
        while sz >= 0x80 {
            size_bytes.push((sz as u8 & 0x7f) | 0x80);
            sz >>= 7;
        }
        size_bytes.push(sz as u8);
        result.extend_from_slice(&size_bytes);
        result.extend_from_slice(&section);
        result
    }

    #[test]
    fn fixture_decodes_exact_entries() {
        let entries = parse_contract_meta(US_NEW_WASM).expect("fixture should parse");
        assert_eq!(
            entries,
            vec![
                ContractMetaEntry {
                    key: "rsver".to_string(),
                    value: "1.97.1".to_string(),
                },
                ContractMetaEntry {
                    key: "rssdkver".to_string(),
                    value: "22.0.11#34f7f53ae31e0fd02aab436a9872e79fa671ca02".to_string(),
                },
                ContractMetaEntry {
                    key: "cliver".to_string(),
                    value: "27.1.0#8e402ea28202950b272fbabc34caad4d2f64fe87".to_string(),
                },
            ]
        );
    }

    #[test]
    fn fixture_metadata_is_attached_to_wasm_metadata() {
        let meta = crate::parse_metadata(US_NEW_WASM).expect("fixture should parse");
        assert_eq!(
            meta.contract_meta,
            parse_contract_meta(US_NEW_WASM).unwrap()
        );
        assert!(!meta.contract_meta.is_empty());
    }

    #[test]
    fn missing_section_is_empty() {
        assert!(parse_contract_meta(US_OLD_WASM).unwrap().is_empty());
        let meta = crate::parse_metadata(US_OLD_WASM).unwrap();
        assert!(meta.contract_meta.is_empty());
    }

    #[test]
    fn empty_input_is_rejected() {
        assert!(matches!(parse_contract_meta(&[]), Err(WasmError::Empty)));
    }

    #[test]
    fn empty_section_payload_is_empty() {
        let wasm = custom_section_wasm(CONTRACT_META_V0, &[]);
        assert!(parse_contract_meta(&wasm).unwrap().is_empty());
    }

    #[test]
    fn truncated_payload_is_classified_error() {
        // Discriminant claims ScMetaV0 but the key/value XDR is missing.
        let wasm = custom_section_wasm(CONTRACT_META_V0, &[0x00, 0x00, 0x00, 0x00, 0x00]);
        let err = parse_contract_meta(&wasm).unwrap_err();
        assert!(matches!(err, WasmError::MetaXdr(_)));
    }

    #[test]
    fn garbage_payload_is_classified_error() {
        let wasm = custom_section_wasm(CONTRACT_META_V0, b"\xff\xff\xff\xff");
        let err = parse_contract_meta(&wasm).unwrap_err();
        assert!(matches!(err, WasmError::MetaXdr(_)));
    }

    #[test]
    fn malformed_payload_does_not_panic_in_parse_metadata() {
        let wasm = custom_section_wasm(CONTRACT_META_V0, &[0xff]);
        let err = crate::parse_metadata(&wasm).unwrap_err();
        assert!(matches!(err, WasmError::MetaXdr(_)));
    }
}
