use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wasmparser::{Parser, Payload};

pub mod abi_decode;
pub mod client_gen;
pub mod spec;
pub mod spec_diff;
pub use abi_decode::{find_event_abi, find_type_abi, format_scval_abi, DecodedValue};
pub use client_gen::{
    generate_client, generate_client_with_options, ClientGenError, GenerateOptions,
};
pub use spec::{
    parse_contract_spec, ContractEvent, ContractFunction, ContractParameter, ContractSpec,
    ContractType,
};
pub use spec_diff::{
    diff_specs, diff_wasm, upgrade_safety, upgrade_safety_wasm, ChangeKind,
    FunctionSignatureChange, SpecDiff, UpgradeVerdict, VerdictChange, WasmSummary,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WasmMetadata {
    pub hash: String,
    pub size_bytes: usize,
    pub version: u16,
    pub exports: Vec<WasmExport>,
    pub imports: Vec<WasmImport>,
    pub custom_sections: Vec<String>,
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
    };

    let parser = Parser::new(0);
    for payload in parser.parse_all(wasm_bytes) {
        match payload? {
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
                meta.custom_sections.push(reader.name().to_string());
            }
            _ => {}
        }
    }

    Ok(meta)
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
}
