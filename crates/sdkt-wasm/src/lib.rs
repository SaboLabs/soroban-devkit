use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wasmparser::{Parser, Payload};

pub mod abi_decode;
pub mod spec;
pub mod spec_diff;
pub use abi_decode::{find_event_abi, find_type_abi, format_scval_abi, DecodedValue};
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
                        kind: format!("{:?}", export.kind),
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
                            kind: format!("{:?}", import.ty),
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
        // Just verify it doesn't crash on empty sections.
        // More complex WASM requires a real binary blob.
        let meta = parse_metadata(WASM_WITH_EXPORTS).unwrap();
        assert_eq!(meta.size_bytes, 11);
        assert!(meta.exports.is_empty());
    }
}
