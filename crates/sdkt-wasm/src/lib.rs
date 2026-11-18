use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wasmparser::{Parser, Payload};

pub mod abi_decode;
pub mod client_gen;
pub mod spec;
pub mod spec_diff;
pub mod token;
pub use abi_decode::{find_event_abi, find_type_abi, format_scval_abi, DecodedValue};
pub use client_gen::{
    generate_client, generate_client_with_options, ClientGenError, GenerateOptions,
};
pub use spec::{
    decode_contract_meta, parse_contract_spec, ContractEvent, ContractFunction, ContractParameter,
    ContractSpec, ContractType, EventParam, TypeMember,
};
pub use spec_diff::{
    diff_specs, diff_wasm, event_sig, size_delta_bytes, size_delta_pct, type_sig, upgrade_safety,
    upgrade_safety_wasm, ChangeKind, EventSignatureChange, FunctionSignatureChange, SpecDiff,
    TypeDefinitionChange, UpgradeVerdict, VerdictChange, WasmSource, WasmSummary,
};
pub use token::{
    classify_all, classify_method, is_token, MethodPresence, Sep41Method, METADATA_METHODS,
    SEP41_METHODS,
};

#[derive(Error, Debug)]
pub enum WasmError {
    #[error("WASM parse error: {0}")]
    Parse(#[from] wasmparser::BinaryReaderError),
    #[error("Empty WASM bytes")]
    Empty,
    #[error("No contractspecv0 section found")]
    NoContractSpec,
    #[error("XDR decode error in contract spec: {0}")]
    SpecXdr(stellar_xdr::Error),
    #[error("XDR decode error in contract metadata: {0}")]
    MetaXdr(stellar_xdr::Error),
}

/// A single `contractmetav0` key/value entry.
///
/// Contract metadata is an arbitrary, open key/value space (the `SCMetaV0`
/// XDR struct is literally a `string key` / `string val` pair), so entries are
/// surfaced verbatim and in section order — no vocabulary is assumed and
/// repeated keys are preserved as separate entries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractMetaEntry {
    /// The metadata key, exactly as declared by the contract (UTF-8).
    pub key: String,
    /// The metadata value, exactly as declared by the contract (UTF-8).
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WasmMetadata {
    pub hash: String,
    pub size_bytes: usize,
    pub version: u16,
    pub exports: Vec<WasmExport>,
    pub imports: Vec<WasmImport>,
    pub custom_sections: Vec<String>,
    /// Decoded `contractmetav0` entries, in section order (additive; may be
    /// empty when the module carries no contract metadata).
    #[serde(default)]
    pub contract_meta: Vec<ContractMetaEntry>,
    /// Number of functions declared in the module's function section.
    #[serde(default)]
    pub function_count: u32,
    /// The first module memory, when one is declared.
    #[serde(default)]
    pub memory: Option<WasmMemory>,
    /// Number of tables declared in the module.
    #[serde(default)]
    pub table_count: u32,
    /// Number of globals declared in the module.
    #[serde(default)]
    pub global_count: u32,
    /// Number of data segments declared in the module.
    #[serde(default)]
    pub data_segment_count: u32,
    /// Per-section byte sizes, in file order (issue #516).
    ///
    /// Each entry is the full file span of one section — its id byte, its
    /// LEB128 size prefix, and its contents — so the entries account for
    /// every byte of the module except the 8-byte module header, which is
    /// not a section: `section_sizes.iter().map(|s| s.size_bytes).sum::<usize>() + 8 == size_bytes`.
    /// A module with no sections has an empty breakdown. The field has a
    /// serde default so payloads serialized before it existed still parse.
    #[serde(default)]
    pub section_sizes: Vec<WasmSectionSize>,
}

/// The byte size of one WASM section, in file order.
///
/// `name` is the canonical section name (`"type"`, `"code"`, `"data"`, …);
/// for a custom section (id 0) it is the custom section's own name, e.g.
/// `"contractspecv0"` or `"contractmetav0"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WasmSectionSize {
    /// Numeric section id (`0` for a custom section).
    pub id: u8,
    /// Section name; the custom section name for id `0`.
    pub name: String,
    /// Byte size of the section in the file, including its id byte and its
    /// LEB128 size prefix.
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WasmMemory {
    pub initial_pages: u64,
    #[serde(default)]
    pub maximum_pages: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WasmExport {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WasmImport {
    pub module: String,
    pub name: String,
    pub kind: String,
}

/// Parse metadata (hash, size, exports, imports, custom sections) from raw
/// Soroban contract WASM bytes.
///
/// # Example
///
/// ```
/// use sdkt_wasm::parse_metadata;
///
/// // Empty input is rejected with `WasmError::Empty`.
/// assert!(parse_metadata(b"").is_err());
/// ```
pub fn parse_metadata(wasm_bytes: &[u8]) -> Result<WasmMetadata, WasmError> {
    if wasm_bytes.is_empty() {
        return Err(WasmError::Empty);
    }

    let mut hasher = Sha256::new();
    hasher.update(wasm_bytes);
    let hash = hex::encode(hasher.finalize());

    let mut meta = WasmMetadata {
        hash,
        size_bytes: wasm_bytes.len(),
        version: 1, // Default, will update if found
        exports: Vec::new(),
        imports: Vec::new(),
        custom_sections: Vec::new(),
        contract_meta: Vec::new(),
        function_count: 0,
        memory: None,
        table_count: 0,
        global_count: 0,
        data_segment_count: 0,
        section_sizes: Vec::new(),
    };

    // Byte offset just past the 8-byte module header; sections are
    // contiguous, so each section's file span runs from the end of the
    // previous section to the end of its own byte range.
    let mut section_cursor: u64 = 8;

    let parser = Parser::new(0);
    for payload in parser.parse_all(wasm_bytes) {
        let payload = payload?;
        // Record the file span of every section before the match below
        // consumes the payload. `as_section` yields the range of the
        // section's contents; the span from the previous section's end
        // adds the section's id byte and LEB128 size prefix.
        if let Some((id, range)) = payload.as_section() {
            let name = match &payload {
                Payload::CustomSection(reader) => reader.name().to_string(),
                _ => section_name(id),
            };
            meta.section_sizes.push(WasmSectionSize {
                id,
                name,
                size_bytes: usize::try_from(range.end.saturating_sub(section_cursor))
                    .unwrap_or(usize::MAX),
            });
            section_cursor = range.end;
        }
        match payload {
            Payload::Version { num, .. } => {
                meta.version = num;
            }
            Payload::ExportSection(reader) => {
                for export_res in reader {
                    let export = export_res?;
                    meta.exports.push(WasmExport {
                        name: export.name.to_string(),
                        kind: export_kind_str(export.kind).to_string(),
                    });
                }
            }
            Payload::ImportSection(reader) => {
                for imports_res in reader {
                    let imports = imports_res?;
                    for imp_res in imports {
                        let (_, import) = imp_res?;
                        meta.imports.push(WasmImport {
                            module: import.module.to_string(),
                            name: import.name.to_string(),
                            kind: import_kind_str(import.ty).to_string(),
                        });
                    }
                }
            }
            Payload::CustomSection(reader) => {
                let name = reader.name().to_string();
                if name == spec::CONTRACT_META_V0 {
                    // Decode the payload as ordered SCMetaEntry key/value pairs
                    // (generic: no vocabulary is assumed). `custom_sections`
                    // keeps recording the section name exactly as before.
                    let entries = spec::decode_contract_meta(reader.data())?;
                    meta.contract_meta.extend(entries);
                }
                meta.custom_sections.push(name);
            }
            Payload::FunctionSection(reader) => {
                meta.function_count = reader.count();
            }
            Payload::MemorySection(reader) => {
                if let Some(Ok(memory)) = reader.into_iter().next() {
                    meta.memory = Some(WasmMemory {
                        initial_pages: memory.initial,
                        maximum_pages: memory.maximum,
                    });
                }
            }
            Payload::TableSection(reader) => {
                meta.table_count = reader.count();
            }
            Payload::GlobalSection(reader) => {
                meta.global_count = reader.count();
            }
            Payload::DataSection(reader) => {
                meta.data_segment_count = reader.count();
            }
            _ => {}
        }
    }

    Ok(meta)
}

/// Canonical name for a WASM section id. Custom sections (id `0`) are
/// named by the caller from the section's own name instead.
fn section_name(id: u8) -> String {
    match id {
        0 => "custom".to_string(),
        1 => "type".to_string(),
        2 => "import".to_string(),
        3 => "function".to_string(),
        4 => "table".to_string(),
        5 => "memory".to_string(),
        6 => "global".to_string(),
        7 => "export".to_string(),
        8 => "start".to_string(),
        9 => "element".to_string(),
        10 => "code".to_string(),
        11 => "data".to_string(),
        12 => "data count".to_string(),
        _ => format!("unknown ({id})"),
    }
}

fn export_kind_str(kind: wasmparser::ExternalKind) -> &'static str {
    match kind {
        wasmparser::ExternalKind::Func => "func",
        wasmparser::ExternalKind::Table => "table",
        wasmparser::ExternalKind::Memory => "memory",
        wasmparser::ExternalKind::Global => "global",
        wasmparser::ExternalKind::Tag => "tag",
        wasmparser::ExternalKind::FuncExact => "func_exact",
    }
}

fn import_kind_str(ty: wasmparser::TypeRef) -> &'static str {
    match ty {
        wasmparser::TypeRef::Func(_) => "func",
        wasmparser::TypeRef::Table(_) => "table",
        wasmparser::TypeRef::Memory(_) => "memory",
        wasmparser::TypeRef::Global(_) => "global",
        wasmparser::TypeRef::Tag(_) => "tag",
        wasmparser::TypeRef::FuncExact(_) => "func_exact",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal valid WASM binary (magic + version 1)
    const VALID_EMPTY_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    // Minimal WASM with an empty export section (section id 7, size 1, 0 items)
    const WASM_WITH_EXPORTS: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 7, 1, 0];

    #[test]
    fn test_empty_bytes() {
        let res = parse_metadata(&[]);
        assert!(matches!(res, Err(WasmError::Empty)));
    }

    #[test]
    fn test_invalid_wasm() {
        let res = parse_metadata(b"not a wasm file");
        assert!(matches!(res, Err(WasmError::Parse(_))));
    }

    #[test]
    fn test_valid_empty_wasm() {
        let meta = parse_metadata(VALID_EMPTY_WASM).unwrap();
        assert_eq!(meta.size_bytes, 8);
        assert_eq!(meta.version, 1);
        assert!(meta.exports.is_empty());
        assert!(meta.imports.is_empty());
        assert!(meta.custom_sections.is_empty());

        let expected_hash = hex::encode(Sha256::digest(VALID_EMPTY_WASM));
        assert_eq!(meta.hash, expected_hash);
    }

    #[test]
    fn test_wasm_exports_parsing() {
        let meta = parse_metadata(WASM_WITH_EXPORTS).unwrap();
        assert_eq!(meta.size_bytes, 11);
        assert!(meta.exports.is_empty());
    }

    #[test]
    fn test_export_kind_str() {
        assert_eq!(export_kind_str(wasmparser::ExternalKind::Func), "func");
        assert_eq!(export_kind_str(wasmparser::ExternalKind::Table), "table");
        assert_eq!(export_kind_str(wasmparser::ExternalKind::Memory), "memory");
        assert_eq!(export_kind_str(wasmparser::ExternalKind::Global), "global");
        assert_eq!(export_kind_str(wasmparser::ExternalKind::Tag), "tag");
        assert_eq!(
            export_kind_str(wasmparser::ExternalKind::FuncExact),
            "func_exact"
        );
    }

    #[test]
    fn test_import_kind_str() {
        assert_eq!(import_kind_str(wasmparser::TypeRef::Func(0)), "func");
        assert_eq!(import_kind_str(wasmparser::TypeRef::Func(42)), "func");
        assert_eq!(
            import_kind_str(wasmparser::TypeRef::Table(wasmparser::TableType {
                element_type: wasmparser::RefType::FUNCREF,
                table64: false,
                initial: 0,
                maximum: None,
                shared: false,
            })),
            "table"
        );
        assert_eq!(
            import_kind_str(wasmparser::TypeRef::Memory(wasmparser::MemoryType {
                initial: 1,
                maximum: None,
                shared: false,
                memory64: false,
                page_size_log2: None,
            })),
            "memory"
        );
        assert_eq!(
            import_kind_str(wasmparser::TypeRef::Global(wasmparser::GlobalType {
                content_type: wasmparser::ValType::I32,
                mutable: false,
                shared: false,
            })),
            "global"
        );
        assert_eq!(
            import_kind_str(wasmparser::TypeRef::Tag(wasmparser::TagType {
                kind: wasmparser::TagKind::Exception,
                func_type_idx: 0,
            })),
            "tag"
        );
        assert_eq!(
            import_kind_str(wasmparser::TypeRef::FuncExact(10)),
            "func_exact"
        );
    }

    #[test]
    fn test_parse_metadata_exports_and_imports_kind() {
        // Minimal WASM binary with:
        // - Type section (section 1): 1 function type () -> ()
        // - Import section (section 2): module "m", name "imp", type 0 (func)
        // - Export section (section 7): name "exp", kind 0 (func), idx 0
        let wasm_bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
            0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type section: 1 type
            0x02, 0x09, 0x01, 0x01, b'm', 0x03, b'i', b'm', b'p', 0x00, 0x00, // import: func
            0x07, 0x07, 0x01, 0x03, b'e', b'x', b'p', 0x00, 0x00, // export: func
        ];
        let meta = parse_metadata(&wasm_bytes).expect("valid wasm bytes");
        assert_eq!(meta.imports.len(), 1);
        assert_eq!(meta.imports[0].module, "m");
        assert_eq!(meta.imports[0].name, "imp");
        assert_eq!(meta.imports[0].kind, "func");

        assert_eq!(meta.exports.len(), 1);
        assert_eq!(meta.exports[0].name, "exp");
        assert_eq!(meta.exports[0].kind, "func");
    }

    // Issue #516: the per-section breakdown. Section sizes are file spans
    // (id byte + LEB128 size prefix + contents), so they account for every
    // byte except the 8-byte module header, which is not a section.
    fn section_sizes_sum(meta: &WasmMetadata) -> usize {
        meta.section_sizes.iter().map(|s| s.size_bytes).sum()
    }

    #[test]
    fn test_section_sizes_empty_wasm() {
        // A module with no sections shows an empty breakdown, not a crash.
        let meta = parse_metadata(VALID_EMPTY_WASM).unwrap();
        assert!(meta.section_sizes.is_empty());
        assert_eq!(meta.size_bytes, 8);
    }

    #[test]
    fn test_section_sizes_export_fixture() {
        // Header (8) + export section: id byte + 1-byte size + 1 content byte.
        let meta = parse_metadata(WASM_WITH_EXPORTS).unwrap();
        assert_eq!(meta.section_sizes.len(), 1);
        assert_eq!(meta.section_sizes[0].id, 7);
        assert_eq!(meta.section_sizes[0].name, "export");
        assert_eq!(meta.section_sizes[0].size_bytes, 3);
        assert_eq!(section_sizes_sum(&meta) + 8, meta.size_bytes);
    }

    #[test]
    fn test_section_sizes_us_old() {
        let wasm_bytes = include_bytes!("../../sdkt-cli/tests/fixtures/us_old.wasm");
        let meta = parse_metadata(wasm_bytes).unwrap();
        assert_eq!(meta.size_bytes, 198);
        assert_eq!(meta.section_sizes.len(), 1);
        assert_eq!(meta.section_sizes[0].id, 0);
        assert_eq!(meta.section_sizes[0].name, "contractspecv0");
        assert_eq!(meta.section_sizes[0].size_bytes, 190);
        assert_eq!(section_sizes_sum(&meta) + 8, meta.size_bytes);
    }

    #[test]
    fn test_section_sizes_us_new() {
        let wasm_bytes = include_bytes!("../../sdkt-cli/tests/fixtures/us_new.wasm");
        let meta = parse_metadata(wasm_bytes).unwrap();
        assert_eq!(meta.size_bytes, 530);
        let names: Vec<&str> = meta.section_sizes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "type",
                "import",
                "function",
                "memory",
                "export",
                "code",
                "contractspecv0",
                "contractenvmetav0",
                "contractmetav0",
                "contractmetav0",
            ]
        );
        assert_eq!(section_sizes_sum(&meta) + 8, meta.size_bytes);
    }
}
