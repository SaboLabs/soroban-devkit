//! ContractSpec ABI parser for Soroban contracts.
//!
//! Reads the `contractspecv0` (and `contractenvmetav0`) custom sections from
//! compiled Soroban WASM and exposes a typed, serializable `ContractSpec`.
//!
//! The `contractspecv0` payload is a sequence of XDR-encoded [`ScSpecEntry`]
//! tuples. Functions, user-defined structs/unions/enums, and events are all
//! discriminated by [`ScSpecEntryKind`].

use serde::{Deserialize, Serialize};
use std::io::Cursor;
use stellar_xdr::{Limited, Limits, ReadXdr, ScSpecEntry, ScSpecEntryKind, ScSpecTypeDef};
use wasmparser::Payload;

use crate::WasmError;

/// Names of the Soroban custom sections this parser understands.
pub const CONTRACT_SPEC_V0: &str = "contractspecv0";
/// Environment metadata section (contract spec version marker).
pub const CONTRACT_ENV_META_V0: &str = "contractenvmetav0";

/// The full contract ABI, as declared in the compiled WASM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractSpec {
    /// Contract-level metadata (from `contractenvmetav0`, currently the
    /// interface version integer).
    pub env_meta: Option<EnvMetaSpec>,
    /// All declared functions.
    pub functions: Vec<ContractFunction>,
    /// All user-defined types (structs, unions, enums, error enums).
    pub custom_types: Vec<ContractType>,
    /// Declared events.
    pub events: Vec<ContractEvent>,
}

/// Parsed `contractenvmetav0` payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvMetaSpec {
    /// Interface version reported by the contract.
    pub interface_version: u64,
}

/// A single exported contract function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractFunction {
    /// Function name (the scSymbol).
    pub name: String,
    /// Doc comment attached to the function, if any.
    pub doc: String,
    /// Ordered input parameters.
    pub parameters: Vec<ContractParameter>,
    /// Ordered output types.
    pub outputs: Vec<ContractType>,
}

/// A named input parameter of a contract function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractParameter {
    /// Parameter name.
    pub name: String,
    /// Doc comment, if any.
    pub doc: String,
    /// Type, expressed as a [`ContractType`].
    pub type_: ContractType,
}

/// A user-defined type declaration (struct / union / enum / error enum).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractType {
    /// Type name.
    pub name: String,
    /// XDR kind (Struct / Union / Enum / ErrorEnum).
    pub kind: String,
    /// Doc comment, if any.
    pub doc: String,
    /// Members (fields for structs, variants for enums/unions).
    pub members: Vec<TypeMember>,
}

/// A member of a user-defined type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeMember {
    /// Member name.
    pub name: String,
    /// Doc comment, if any.
    pub doc: String,
}

/// A declared Soroban event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractEvent {
    /// Event name.
    pub name: String,
    /// Doc comment, if any.
    pub doc: String,
}

/// Parses the Soroban contract spec from compiled WASM bytes.
///
/// This reuses the same [`wasmparser::Parser`] walk as [`crate::parse_metadata`]
/// but extracts and decodes the `contractspecv0` / `contractenvmetav0` custom
/// section payloads.
///
/// # Errors
///
/// Returns [`WasmError::Empty`] for empty input, [`WasmError::Parse`] for
/// malformed WASM or a malformed spec section, and
/// [`WasmError::NoContractSpec`] when the WASM is valid but declares no
/// contract spec (i.e. it is not a Soroban contract).
pub fn parse_contract_spec(raw_wasm: &[u8]) -> Result<ContractSpec, WasmError> {
    if raw_wasm.is_empty() {
        return Err(WasmError::Empty);
    }

    let parser = wasmparser::Parser::new(0);
    let mut functions = Vec::new();
    let mut custom_types = Vec::new();
    let mut events = Vec::new();
    let mut env_meta: Option<EnvMetaSpec> = None;
    let mut saw_spec = false;

    for payload in parser.parse_all(raw_wasm) {
        if let Payload::CustomSection(reader) = payload? {
            match reader.name() {
                CONTRACT_SPEC_V0 => {
                    saw_spec = true;
                    decode_spec_section(
                        reader.data(),
                        &mut functions,
                        &mut custom_types,
                        &mut events,
                    )?;
                }
                CONTRACT_ENV_META_V0 => {
                    env_meta = Some(decode_env_meta_section(reader.data())?);
                }
                _ => {}
            }
        }
    }

    if !saw_spec {
        return Err(WasmError::NoContractSpec);
    }

    Ok(ContractSpec {
        env_meta,
        functions,
        custom_types,
        events,
    })
}

/// Decodes a sequence of XDR `ScSpecEntry` values into the typed model.
fn decode_spec_section(
    data: &[u8],
    functions: &mut Vec<ContractFunction>,
    custom_types: &mut Vec<ContractType>,
    events: &mut Vec<ContractEvent>,
) -> Result<(), WasmError> {
    let mut items = data;
    // A spec section may be either a single ScSpecEntry or a set of
    // concatenated entries. `ScSpecEntry::read_xdr` consumes one entry at a
    // time, so we loop over the buffer until no bytes remain (or an XDR error).
    while !items.is_empty() {
        let mut cursor = Cursor::new(items);
        let mut limited = Limited::new(&mut cursor, Limits::none());
        let entry = ScSpecEntry::read_xdr(&mut limited).map_err(WasmError::SpecXdr)?;

        match entry {
            ScSpecEntry::FunctionV0(f) => {
                functions.push(ContractFunction {
                    name: f.name.to_utf8_string_lossy(),
                    doc: f.doc.to_utf8_string_lossy(),
                    parameters: f
                        .inputs
                        .iter()
                        .map(|i| ContractParameter {
                            name: i.name.to_utf8_string_lossy(),
                            doc: i.doc.to_utf8_string_lossy(),
                            type_: map_type_def(&i.type_),
                        })
                        .collect(),
                    outputs: f.outputs.iter().map(map_type_def).collect(),
                });
            }
            ScSpecEntry::UdtStructV0(s) => custom_types.push(map_udt_struct(s)),
            ScSpecEntry::UdtUnionV0(u) => custom_types.push(map_udt_union(u)),
            ScSpecEntry::UdtEnumV0(e) => custom_types.push(map_udt_enum(e)),
            ScSpecEntry::UdtErrorEnumV0(e) => custom_types.push(map_udt_error_enum(e)),
            ScSpecEntry::EventV0(e) => events.push(ContractEvent {
                name: e.name.to_utf8_string_lossy(),
                doc: e.doc.to_utf8_string_lossy(),
            }),
        }

        let consumed = cursor.position() as usize;
        items = &items[consumed..];
    }
    Ok(())
}

/// Decodes the `contractenvmetav0` payload (a single `u64` interface version).
fn decode_env_meta_section(data: &[u8]) -> Result<EnvMetaSpec, WasmError> {
    if data.len() < 8 {
        return Err(WasmError::SpecXdr(stellar_xdr::Error::Invalid));
    }
    let raw = [
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ];
    // ReadXdr for Uint64 is big-endian like Stellar XDR.
    let mut cursor = Cursor::new(&raw[..]);
    let mut limited = Limited::new(&mut cursor, Limits::none());
    let v = stellar_xdr::Uint64::read_xdr(&mut limited).map_err(WasmError::SpecXdr)?;
    Ok(EnvMetaSpec {
        interface_version: v,
    })
}

fn map_type_def(t: &ScSpecTypeDef) -> ContractType {
    // For simple type definitions there is no doc/members; we encode the
    // variant name into `name` and `kind` for introspection.
    let mut udt_name: Option<String> = None;
    let (name, kind, members) = match t {
        ScSpecTypeDef::Val => ("val", "primitive", vec![]),
        ScSpecTypeDef::Bool => ("bool", "primitive", vec![]),
        ScSpecTypeDef::Void => ("void", "primitive", vec![]),
        ScSpecTypeDef::Error => ("error", "primitive", vec![]),
        ScSpecTypeDef::U32 => ("u32", "primitive", vec![]),
        ScSpecTypeDef::I32 => ("i32", "primitive", vec![]),
        ScSpecTypeDef::U64 => ("u64", "primitive", vec![]),
        ScSpecTypeDef::I64 => ("i64", "primitive", vec![]),
        ScSpecTypeDef::Timepoint => ("timepoint", "primitive", vec![]),
        ScSpecTypeDef::Duration => ("duration", "primitive", vec![]),
        ScSpecTypeDef::U128 => ("u128", "primitive", vec![]),
        ScSpecTypeDef::I128 => ("i128", "primitive", vec![]),
        ScSpecTypeDef::U256 => ("u256", "primitive", vec![]),
        ScSpecTypeDef::I256 => ("i256", "primitive", vec![]),
        ScSpecTypeDef::Bytes => ("bytes", "primitive", vec![]),
        ScSpecTypeDef::String => ("string", "primitive", vec![]),
        ScSpecTypeDef::Symbol => ("symbol", "primitive", vec![]),
        ScSpecTypeDef::Address => ("address", "primitive", vec![]),
        ScSpecTypeDef::MuxedAddress => ("muxed_address", "primitive", vec![]),
        ScSpecTypeDef::Option(_) => ("option", "compound", vec![]),
        ScSpecTypeDef::Result(_) => ("result", "compound", vec![]),
        ScSpecTypeDef::Vec(_) => ("vec", "compound", vec![]),
        ScSpecTypeDef::Map(_) => ("map", "compound", vec![]),
        ScSpecTypeDef::Tuple(_) => ("tuple", "compound", vec![]),
        ScSpecTypeDef::BytesN(_) => ("bytesn", "compound", vec![]),
        ScSpecTypeDef::Udt(u) => {
            udt_name = Some(u.name.to_utf8_string_lossy());
            ("udt", "udt", vec![])
        }
    };
    ContractType {
        name: udt_name.unwrap_or_else(|| name.to_string()),
        kind: kind.to_string(),
        doc: String::new(),
        members,
    }
}

fn map_udt_struct(s: stellar_xdr::ScSpecUdtStructV0) -> ContractType {
    ContractType {
        name: s.name.to_utf8_string_lossy(),
        kind: "struct".to_string(),
        doc: s.doc.to_utf8_string_lossy(),
        members: s
            .fields
            .iter()
            .map(|f| TypeMember {
                name: f.name.to_utf8_string_lossy(),
                doc: f.doc.to_utf8_string_lossy(),
            })
            .collect(),
    }
}

fn map_udt_union(u: stellar_xdr::ScSpecUdtUnionV0) -> ContractType {
    ContractType {
        name: u.name.to_utf8_string_lossy(),
        kind: "union".to_string(),
        doc: u.doc.to_utf8_string_lossy(),
        members: u
            .cases
            .iter()
            .map(|c| match c {
                stellar_xdr::ScSpecUdtUnionCaseV0::VoidV0(v) => TypeMember {
                    name: v.name.to_utf8_string_lossy(),
                    doc: v.doc.to_utf8_string_lossy(),
                },
                stellar_xdr::ScSpecUdtUnionCaseV0::TupleV0(t) => TypeMember {
                    name: t.name.to_utf8_string_lossy(),
                    doc: t.doc.to_utf8_string_lossy(),
                },
            })
            .collect(),
    }
}

fn map_udt_enum(e: stellar_xdr::ScSpecUdtEnumV0) -> ContractType {
    ContractType {
        name: e.name.to_utf8_string_lossy(),
        kind: "enum".to_string(),
        doc: e.doc.to_utf8_string_lossy(),
        members: e
            .cases
            .iter()
            .map(|c| TypeMember {
                name: c.name.to_utf8_string_lossy(),
                doc: c.doc.to_utf8_string_lossy(),
            })
            .collect(),
    }
}

fn map_udt_error_enum(e: stellar_xdr::ScSpecUdtErrorEnumV0) -> ContractType {
    ContractType {
        name: e.name.to_utf8_string_lossy(),
        kind: "error_enum".to_string(),
        doc: e.doc.to_utf8_string_lossy(),
        members: e
            .cases
            .iter()
            .map(|c| TypeMember {
                name: c.name.to_utf8_string_lossy(),
                doc: c.doc.to_utf8_string_lossy(),
            })
            .collect(),
    }
}

// Keep `ScSpecEntryKind` referenced to avoid unused-import warnings on some
// feature combinations.
#[allow(dead_code)]
fn _discriminant_name(kind: &ScSpecEntryKind) -> &'static str {
    match kind {
        ScSpecEntryKind::FunctionV0 => "function_v0",
        ScSpecEntryKind::UdtStructV0 => "udt_struct_v0",
        ScSpecEntryKind::UdtUnionV0 => "udt_union_v0",
        ScSpecEntryKind::UdtEnumV0 => "udt_enum_v0",
        ScSpecEntryKind::UdtErrorEnumV0 => "udt_error_enum_v0",
        ScSpecEntryKind::EventV0 => "event_v0",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::WriteXdr;

    /// Minimal valid WASM (magic + version 1).
    const VALID_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    /// Encodes `ScSpecEntry` values into a `contractspecv0` custom section.
    fn spec_section(entries: &[ScSpecEntry]) -> Vec<u8> {
        // Build the custom section payload: name + encoded XDR entries.
        let mut section = Vec::new();
        section.push(CONTRACT_SPEC_V0.len() as u8);
        section.extend_from_slice(CONTRACT_SPEC_V0.as_bytes());
        for e in entries {
            let mut buf = Vec::new();
            let mut cursor = Cursor::new(&mut buf);
            let mut l = Limited::new(&mut cursor, Limits::none());
            e.write_xdr(&mut l).unwrap();
            section.extend_from_slice(&buf);
        }
        // Assemble: WASM magic + version, custom-section id(0), uleb128 size, payload.
        let mut result = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        result.push(0); // section id
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

    fn symbol_e(s: &str) -> stellar_xdr::ScSymbol {
        stellar_xdr::ScSymbol(s.to_string().try_into().unwrap())
    }

    fn func_entry(name: &str, inputs: Vec<(String, ScSpecTypeDef)>) -> ScSpecEntry {
        use stellar_xdr::{ScSpecFunctionInputV0, ScSpecFunctionV0};
        ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
            doc: "".try_into().unwrap(),
            name: symbol_e(name),
            inputs: inputs
                .into_iter()
                .map(|(n, t)| ScSpecFunctionInputV0 {
                    doc: "".try_into().unwrap(),
                    name: n.try_into().unwrap(),
                    type_: t,
                })
                .collect::<Vec<_>>()
                .try_into()
                .unwrap(),
            outputs: vec![].try_into().unwrap(),
        })
    }

    #[test]
    fn empty_wasm() {
        assert!(matches!(parse_contract_spec(&[]), Err(WasmError::Empty)));
    }

    #[test]
    fn invalid_wasm() {
        let res = parse_contract_spec(b"not wasm at all");
        assert!(matches!(res, Err(WasmError::Parse(_))));
    }

    #[test]
    fn no_contract_spec() {
        // Valid WASM without a `contractspecv0` section.
        let res = parse_contract_spec(VALID_WASM);
        assert!(matches!(res, Err(WasmError::NoContractSpec)));
    }

    #[test]
    fn valid_single_function() {
        let wasm = spec_section(&[func_entry(
            "greet",
            vec![("name".to_string(), ScSpecTypeDef::String)],
        )]);
        let spec = parse_contract_spec(&wasm).unwrap();
        assert_eq!(spec.functions.len(), 1);
        assert_eq!(spec.functions[0].name, "greet");
        assert_eq!(spec.functions[0].parameters.len(), 1);
        assert_eq!(spec.functions[0].parameters[0].name, "name");
        assert_eq!(spec.functions[0].parameters[0].type_.name, "string");
    }

    #[test]
    fn multiple_functions() {
        let wasm = spec_section(&[
            func_entry("a", vec![]),
            func_entry("b", vec![("x".to_string(), ScSpecTypeDef::U32)]),
        ]);
        let spec = parse_contract_spec(&wasm).unwrap();
        assert_eq!(spec.functions.len(), 2);
        assert_eq!(spec.functions[0].name, "a");
        assert_eq!(spec.functions[1].name, "b");
        assert_eq!(spec.functions[1].parameters[0].type_.name, "u32");
    }

    #[test]
    fn custom_type() {
        use stellar_xdr::ScSpecUdtStructV0;
        let udt = ScSpecEntry::UdtStructV0(ScSpecUdtStructV0 {
            doc: "a point".try_into().unwrap(),
            lib: "soroban_sdk".try_into().unwrap(),
            name: "Point".try_into().unwrap(),
            fields: vec![].try_into().unwrap(),
        });
        let wasm = spec_section(&[udt]);
        let spec = parse_contract_spec(&wasm).unwrap();
        assert_eq!(spec.custom_types.len(), 1);
        assert_eq!(spec.custom_types[0].name, "Point");
        assert_eq!(spec.custom_types[0].kind, "struct");
        assert_eq!(spec.custom_types[0].doc, "a point");
    }
}
