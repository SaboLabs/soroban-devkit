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
use stellar_xdr::{
    Limited, Limits, ReadXdr, ScEnvMetaEntry, ScMetaEntry, ScSpecEntry, ScSpecEntryKind,
    ScSpecTypeDef,
};
use wasmparser::Payload;

use crate::{ContractMetaEntry, WasmError};

/// Names of the Soroban custom sections this parser understands.
pub const CONTRACT_SPEC_V0: &str = "contractspecv0";
/// Environment metadata section (contract spec version marker).
pub const CONTRACT_ENV_META_V0: &str = "contractenvmetav0";
/// Contract metadata section (arbitrary key/value `SCMetaEntry` pairs, SEP-46).
pub const CONTRACT_META_V0: &str = "contractmetav0";

/// Nesting cap for untrusted spec payloads.
///
/// `Limits::none()` sets `depth: u32::MAX`, so a crafted `contractspecv0`
/// section nesting `ScSpecTypeDef` thousands of levels deep drives the
/// recursive XDR decoder into the Rust stack limit and aborts with `SIGABRT`
/// — an unrecoverable crash rather than a [`WasmError`]. stellar-xdr documents
/// the depth limit as the defence against exactly that.
///
/// 500 matches the precedent already set on the transaction surface
/// (`sdkt-xdr`'s `decode_envelope`), and is far above any real contract: the
/// deepest spec types in the wild nest a handful of levels.
pub const MAX_SPEC_XDR_DEPTH: u32 = 500;

/// The full contract ABI, as declared in the compiled WASM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractSpec {
    /// Contract-level metadata (from `contractenvmetav0`). The section is the
    /// `SCEnvMetaEntry` union; the only defined variant carries a `protocol`
    /// version and a `pre_release` marker. `None` when the WASM declares no
    /// `contractenvmetav0` section. The upgrade-safety diff does not consume
    /// these fields — see [`crate::spec_diff`].
    pub env_meta: Option<EnvMetaSpec>,
    /// All declared functions.
    pub functions: Vec<ContractFunction>,
    /// All user-defined types (structs, unions, enums, error enums).
    pub custom_types: Vec<ContractType>,
    /// Declared events.
    pub events: Vec<ContractEvent>,
}

/// Parsed `contractenvmetav0` payload.
///
/// The XDR definition (`SCEnvMetaEntry`, stellar-xdr 28.0.0) is a single-arm
/// union:
///
/// ```text
/// union SCEnvMetaEntry switch (SCEnvMetaKind kind)
/// {
/// case SC_ENV_META_KIND_INTERFACE_VERSION:
///     struct {
///         uint32 protocol;
///         uint32 preRelease;
///     } interfaceVersion;
/// };
/// ```
///
/// `protocol` is the ledger/protocol version checked for ordered host
/// compatibility (soroban-env-common `meta.rs`); `pre_release` is a value
/// checked for exact identity. There is no "interface version" integer on the
/// wire — the union arm is merely *named* `interfaceVersion`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvMetaSpec {
    /// Protocol version (`SCEnvMetaEntryInterfaceVersion.protocol`).
    pub protocol: u32,
    /// Pre-release marker (`SCEnvMetaEntryInterfaceVersion.preRelease`).
    pub pre_release: u32,
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
    /// Type arguments for compound types such as `Vec<T>`, `Map<K, V>`, and
    /// `Result<T, E>`.
    #[serde(default)]
    pub type_args: Vec<ContractType>,
    /// Length for the fixed-size `BytesN` type.
    #[serde(default)]
    pub bytes_n: Option<u32>,
}

/// A member of a user-defined type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeMember {
    /// Member name.
    pub name: String,
    /// Doc comment, if any.
    pub doc: String,
    /// The member's type(s). A struct field has exactly one; a union tuple case
    /// carries one per tuple element; void union cases and enum cases have
    /// none. Retained so a type whose members keep their names but change their
    /// types is still detectable as a definition change.
    pub types: Vec<ContractType>,
    /// The case's discriminant, for enum and error-enum members only. This is
    /// the numeric value an `ScVal` carries for the case, so a remapped
    /// discriminant silently changes how existing data decodes.
    pub value: Option<u32>,
}

/// A parameter declared by a Soroban event.
pub type ContractEventParameter = EventParam;

/// A declared Soroban event.
///
/// The full XDR signature (`prefix_topics`, `params`, `data_format`) is
/// retained so that an event which keeps its name but changes its shape is
/// still comparable — see [`crate::spec_diff`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractEvent {
    /// Event name.
    pub name: String,
    /// Doc comment, if any.
    pub doc: String,
    /// Topic symbols emitted before the declared params.
    pub prefix_topics: Vec<String>,
    /// Ordered event parameters, each carrying its topic-list-vs-data location.
    pub params: Vec<EventParam>,
    /// XDR data format (`single_value` / `vec` / `map`).
    pub data_format: String,
}

/// A single parameter of a declared event (`ScSpecEventParamV0`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventParam {
    /// Parameter name.
    pub name: String,
    /// Doc comment, if any.
    pub doc: String,
    /// Type, expressed as a [`ContractType`].
    pub type_: ContractType,
    /// Where the value is carried: `data` or `topic_list`.
    pub location: String,
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
        let mut limited = Limited::new(
            &mut cursor,
            Limits {
                depth: MAX_SPEC_XDR_DEPTH,
                len: items.len(),
            },
        );
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
            ScSpecEntry::EventV0(e) => events.push(map_event(e)),
        }

        let consumed = cursor.position() as usize;
        items = &items[consumed..];
    }
    Ok(())
}

/// Decodes the `contractenvmetav0` payload as the `SCEnvMetaEntry` union.
///
/// The payload is exactly 12 bytes: a 4-byte discriminant `kind`, then a
/// 4-byte `protocol`, then a 4-byte `pre_release`. The only defined variant
/// is `ScEnvMetaKindInterfaceVersion` (kind = 0); any other kind is rejected
/// with [`WasmError::MetaXdr`] rather than folded into a numeric field.
/// A payload shorter than 12 bytes (including the 8-byte truncated form) is
/// also rejected.
fn decode_env_meta_section(data: &[u8]) -> Result<EnvMetaSpec, WasmError> {
    if data.len() != 12 {
        return Err(WasmError::MetaXdr(stellar_xdr::Error::Invalid));
    }
    let mut cursor = Cursor::new(data);
    let mut limited = Limited::new(
        &mut cursor,
        Limits {
            depth: MAX_SPEC_XDR_DEPTH,
            len: data.len(),
        },
    );
    let entry = ScEnvMetaEntry::read_xdr(&mut limited).map_err(WasmError::MetaXdr)?;
    match entry {
        ScEnvMetaEntry::ScEnvMetaKindInterfaceVersion(v) => Ok(EnvMetaSpec {
            protocol: v.protocol,
            pre_release: v.pre_release,
        }),
    }
}

/// Decodes the `contractmetav0` payload into ordered key/value entries.
///
/// The section is a sequence of concatenated XDR `SCMetaEntry` values (each a
/// `SCMetaV0 { string key; string val; }` union arm). The key/value vocabulary
/// is open — SEP-58 continues to evolve it (`rsver`, `rssdkver`, `cliver`,
/// `bldimg`, `source_repo`/`source_rev`, and newer names) — so this decoder
/// performs no vocabulary matching and preserves duplicates and order.
///
/// A present-but-empty section yields an empty list. A payload that does not
/// decode cleanly as XDR is surfaced as [`WasmError::MetaXdr`] rather than
/// silently dropped, so a corrupted section cannot masquerade as "no metadata".
pub fn decode_contract_meta(data: &[u8]) -> Result<Vec<ContractMetaEntry>, WasmError> {
    let mut entries = Vec::new();
    let mut items = data;
    while !items.is_empty() {
        let mut cursor = Cursor::new(items);
        let mut limited = Limited::new(
            &mut cursor,
            Limits {
                depth: MAX_SPEC_XDR_DEPTH,
                len: items.len(),
            },
        );
        let entry = ScMetaEntry::read_xdr(&mut limited).map_err(WasmError::MetaXdr)?;

        match entry {
            ScMetaEntry::ScMetaV0(v0) => entries.push(ContractMetaEntry {
                key: v0.key.to_utf8_string_lossy(),
                value: v0.val.to_utf8_string_lossy(),
            }),
        }

        let consumed = cursor.position() as usize;
        items = &items[consumed..];
    }
    Ok(entries)
}

fn map_type_def(t: &ScSpecTypeDef) -> ContractType {
    fn primitive(name: &str) -> ContractType {
        ContractType {
            name: name.to_string(),
            kind: "primitive".to_string(),
            doc: String::new(),
            members: vec![],
            type_args: vec![],
            bytes_n: None,
        }
    }

    fn compound(name: String, type_args: Vec<ContractType>, labels: &[&str]) -> ContractType {
        let members = type_args
            .iter()
            .zip(labels)
            .map(|(ty, label)| TypeMember {
                name: (*label).to_string(),
                doc: String::new(),
                types: vec![ty.clone()],
                value: None,
            })
            .collect();
        ContractType {
            name,
            kind: "compound".to_string(),
            doc: String::new(),
            members,
            type_args,
            bytes_n: None,
        }
    }

    match t {
        ScSpecTypeDef::Val => primitive("val"),
        ScSpecTypeDef::Bool => primitive("bool"),
        ScSpecTypeDef::Void => primitive("void"),
        ScSpecTypeDef::Error => primitive("error"),
        ScSpecTypeDef::U32 => primitive("u32"),
        ScSpecTypeDef::I32 => primitive("i32"),
        ScSpecTypeDef::U64 => primitive("u64"),
        ScSpecTypeDef::I64 => primitive("i64"),
        ScSpecTypeDef::Timepoint => primitive("timepoint"),
        ScSpecTypeDef::Duration => primitive("duration"),
        ScSpecTypeDef::U128 => primitive("u128"),
        ScSpecTypeDef::I128 => primitive("i128"),
        ScSpecTypeDef::U256 => primitive("u256"),
        ScSpecTypeDef::I256 => primitive("i256"),
        ScSpecTypeDef::Bytes => primitive("bytes"),
        ScSpecTypeDef::String => primitive("string"),
        ScSpecTypeDef::Symbol => primitive("symbol"),
        ScSpecTypeDef::Address => primitive("address"),
        ScSpecTypeDef::MuxedAddress => primitive("muxed_address"),
        ScSpecTypeDef::Option(inner) => {
            let args = vec![map_type_def(&inner.value_type)];
            compound(format!("option<{}>", args[0].name), args, &["value_type"])
        }
        ScSpecTypeDef::Result(inner) => {
            let args = vec![
                map_type_def(&inner.ok_type),
                map_type_def(&inner.error_type),
            ];
            let name = format!("result<{}, {}>", args[0].name, args[1].name);
            compound(name, args, &["ok_type", "error_type"])
        }
        ScSpecTypeDef::Vec(inner) => {
            let args = vec![map_type_def(&inner.element_type)];
            compound(format!("vec<{}>", args[0].name), args, &["element_type"])
        }
        ScSpecTypeDef::Map(inner) => {
            let args = vec![
                map_type_def(&inner.key_type),
                map_type_def(&inner.value_type),
            ];
            let name = format!("map<{}, {}>", args[0].name, args[1].name);
            compound(name, args, &["key_type", "value_type"])
        }
        ScSpecTypeDef::Tuple(inner) => {
            let args: Vec<_> = inner.value_types.iter().map(map_type_def).collect();
            let name = format!(
                "tuple<{}>",
                args.iter()
                    .map(|ty| ty.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let labels: Vec<_> = (0..args.len()).map(|i| i.to_string()).collect();
            let members = args
                .iter()
                .zip(labels.iter())
                .map(|(ty, label)| TypeMember {
                    name: label.clone(),
                    doc: String::new(),
                    types: vec![ty.clone()],
                    value: None,
                })
                .collect();
            ContractType {
                name,
                kind: "compound".to_string(),
                doc: String::new(),
                members,
                type_args: args,
                bytes_n: None,
            }
        }
        ScSpecTypeDef::BytesN(inner) => ContractType {
            name: format!("bytesn<{}>", inner.n),
            kind: "compound".to_string(),
            doc: String::new(),
            members: vec![],
            type_args: vec![],
            bytes_n: Some(inner.n),
        },
        ScSpecTypeDef::Udt(user_type) => ContractType {
            name: user_type.name.to_utf8_string_lossy(),
            kind: "udt".to_string(),
            doc: String::new(),
            members: vec![],
            type_args: vec![],
            bytes_n: None,
        },
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
                types: vec![map_type_def(&f.type_)],
                value: None,
            })
            .collect(),
        type_args: vec![],
        bytes_n: None,
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
                    types: vec![],
                    value: None,
                },
                stellar_xdr::ScSpecUdtUnionCaseV0::TupleV0(t) => TypeMember {
                    name: t.name.to_utf8_string_lossy(),
                    doc: t.doc.to_utf8_string_lossy(),
                    types: t.type_.iter().map(map_type_def).collect(),
                    value: None,
                },
            })
            .collect(),
        type_args: vec![],
        bytes_n: None,
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
                types: vec![],
                value: Some(c.value),
            })
            .collect(),
        type_args: vec![],
        bytes_n: None,
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
                types: vec![],
                value: Some(c.value),
            })
            .collect(),
        type_args: vec![],
        bytes_n: None,
    }
}

/// Maps an `EventV0` entry, retaining the full event signature (prefix
/// topics, params, and data format) rather than just the name.
fn map_event(e: stellar_xdr::ScSpecEventV0) -> ContractEvent {
    ContractEvent {
        name: e.name.to_utf8_string_lossy(),
        doc: e.doc.to_utf8_string_lossy(),
        prefix_topics: e
            .prefix_topics
            .iter()
            .map(|t| t.to_utf8_string_lossy())
            .collect(),
        params: e
            .params
            .iter()
            .map(|p| EventParam {
                name: p.name.to_utf8_string_lossy(),
                doc: p.doc.to_utf8_string_lossy(),
                type_: map_type_def(&p.type_),
                location: match p.location {
                    stellar_xdr::ScSpecEventParamLocationV0::Data => "data".to_string(),
                    stellar_xdr::ScSpecEventParamLocationV0::TopicList => "topic_list".to_string(),
                },
            })
            .collect(),
        data_format: match e.data_format {
            stellar_xdr::ScSpecEventDataFormat::SingleValue => "single_value".to_string(),
            stellar_xdr::ScSpecEventDataFormat::Vec => "vec".to_string(),
            stellar_xdr::ScSpecEventDataFormat::Map => "map".to_string(),
        },
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
pub(crate) mod tests {
    use super::*;
    use stellar_xdr::WriteXdr;

    /// Minimal valid WASM (magic + version 1).
    const VALID_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    /// Encodes `ScSpecEntry` values into a `contractspecv0` custom section.
    pub(crate) fn spec_section(entries: &[ScSpecEntry]) -> Vec<u8> {
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

    pub(crate) fn symbol_e(s: &str) -> stellar_xdr::ScSymbol {
        stellar_xdr::ScSymbol(s.to_string().try_into().unwrap())
    }

    pub(crate) fn func_entry(name: &str, inputs: Vec<(String, ScSpecTypeDef)>) -> ScSpecEntry {
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

    pub(crate) fn event_entry(name: &str) -> ScSpecEntry {
        event_entry_with_params(name, vec![])
    }

    pub(crate) fn event_entry_with_params(
        name: &str,
        params: Vec<(String, ScSpecTypeDef)>,
    ) -> ScSpecEntry {
        use stellar_xdr::{ScSpecEventParamLocationV0, ScSpecEventParamV0, ScSpecEventV0};
        ScSpecEntry::EventV0(ScSpecEventV0 {
            doc: "".try_into().unwrap(),
            lib: "soroban_sdk".try_into().unwrap(),
            name: symbol_e(name),
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
            data_format: stellar_xdr::ScSpecEventDataFormat::SingleValue,
        })
    }

    pub(crate) fn udt_struct_entry(name: &str) -> ScSpecEntry {
        udt_struct_entry_with_fields(name, vec![])
    }

    pub(crate) fn udt_struct_entry_with_fields(
        name: &str,
        fields: Vec<(String, ScSpecTypeDef)>,
    ) -> ScSpecEntry {
        use stellar_xdr::{ScSpecUdtStructFieldV0, ScSpecUdtStructV0};
        ScSpecEntry::UdtStructV0(ScSpecUdtStructV0 {
            doc: "".try_into().unwrap(),
            lib: "soroban_sdk".try_into().unwrap(),
            name: name.try_into().unwrap(),
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

    #[test]
    fn preserves_struct_union_and_event_metadata() {
        use stellar_xdr::{
            ScSpecEventDataFormat, ScSpecEventParamLocationV0, ScSpecEventParamV0, ScSpecEventV0,
            ScSpecUdtStructFieldV0, ScSpecUdtStructV0, ScSpecUdtUnionCaseTupleV0,
            ScSpecUdtUnionCaseV0, ScSpecUdtUnionV0,
        };

        let point = ScSpecEntry::UdtStructV0(ScSpecUdtStructV0 {
            doc: "point".try_into().unwrap(),
            lib: "test".try_into().unwrap(),
            name: "Point".try_into().unwrap(),
            fields: vec![ScSpecUdtStructFieldV0 {
                doc: "x coordinate".try_into().unwrap(),
                name: "x".try_into().unwrap(),
                type_: ScSpecTypeDef::U64,
            }]
            .try_into()
            .unwrap(),
        });
        let choice = ScSpecEntry::UdtUnionV0(ScSpecUdtUnionV0 {
            doc: "choice".try_into().unwrap(),
            lib: "test".try_into().unwrap(),
            name: "Choice".try_into().unwrap(),
            cases: vec![ScSpecUdtUnionCaseV0::TupleV0(ScSpecUdtUnionCaseTupleV0 {
                doc: "some value".try_into().unwrap(),
                name: "Some".try_into().unwrap(),
                type_: vec![ScSpecTypeDef::String].try_into().unwrap(),
            })]
            .try_into()
            .unwrap(),
        });
        let event = ScSpecEntry::EventV0(ScSpecEventV0 {
            doc: "transfer event".try_into().unwrap(),
            lib: "test".try_into().unwrap(),
            name: symbol_e("Transfer"),
            prefix_topics: vec![symbol_e("TOKEN")].try_into().unwrap(),
            params: vec![ScSpecEventParamV0 {
                doc: "amount".try_into().unwrap(),
                name: "amount".try_into().unwrap(),
                type_: ScSpecTypeDef::U128,
                location: ScSpecEventParamLocationV0::Data,
            }]
            .try_into()
            .unwrap(),
            data_format: ScSpecEventDataFormat::SingleValue,
        });

        let spec = parse_contract_spec(&spec_section(&[point, choice, event])).unwrap();
        assert_eq!(spec.custom_types[0].members[0].types[0].name, "u64");
        assert_eq!(spec.custom_types[1].members[0].types[0].name, "string");
        assert_eq!(spec.events[0].prefix_topics, vec!["TOKEN"]);
        assert_eq!(spec.events[0].params[0].type_.name, "u128");
        assert_eq!(spec.events[0].params[0].location, "data");
        assert_eq!(spec.events[0].data_format, "single_value");
    }

    #[test]
    fn preserves_compound_type_arguments_and_bytesn_length() {
        use stellar_xdr::{ScSpecTypeBytesN, ScSpecTypeDef, ScSpecTypeMap, ScSpecTypeVec};

        let vector = map_type_def(&ScSpecTypeDef::Vec(Box::new(ScSpecTypeVec {
            element_type: Box::new(ScSpecTypeDef::U32),
        })));
        assert_eq!(vector.name, "vec<u32>");
        assert_eq!(vector.type_args.len(), 1);
        assert_eq!(vector.type_args[0].name, "u32");

        let map = map_type_def(&ScSpecTypeDef::Map(Box::new(ScSpecTypeMap {
            key_type: Box::new(ScSpecTypeDef::String),
            value_type: Box::new(ScSpecTypeDef::Address),
        })));
        assert_eq!(
            map.type_args
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["string", "address"]
        );

        let bytes = map_type_def(&ScSpecTypeDef::BytesN(ScSpecTypeBytesN { n: 32 }));
        assert_eq!(bytes.name, "bytesn<32>");
        assert_eq!(bytes.bytes_n, Some(32));
    }

    #[test]
    fn event_retains_params() {
        // An `EventV0` entry keeps its full signature, not just its name.
        let wasm = spec_section(&[event_entry_with_params(
            "Transfer",
            vec![
                ("from".to_string(), ScSpecTypeDef::Address),
                ("amount".to_string(), ScSpecTypeDef::I128),
            ],
        )]);
        let spec = parse_contract_spec(&wasm).unwrap();
        assert_eq!(spec.events.len(), 1);
        let ev = &spec.events[0];
        assert_eq!(ev.name, "Transfer");
        assert_eq!(ev.data_format, "single_value");
        assert!(ev.prefix_topics.is_empty());
        assert_eq!(ev.params.len(), 2);
        assert_eq!(ev.params[0].name, "from");
        assert_eq!(ev.params[0].type_.name, "address");
        assert_eq!(ev.params[0].location, "data");
        assert_eq!(ev.params[1].name, "amount");
        assert_eq!(ev.params[1].type_.name, "i128");
    }

    #[test]
    fn udt_struct_retains_field_types() {
        let wasm = spec_section(&[udt_struct_entry_with_fields(
            "Point",
            vec![("x".to_string(), ScSpecTypeDef::I32)],
        )]);
        let spec = parse_contract_spec(&wasm).unwrap();
        assert_eq!(spec.custom_types.len(), 1);
        assert_eq!(spec.custom_types[0].members.len(), 1);
        assert_eq!(spec.custom_types[0].members[0].name, "x");
        assert_eq!(spec.custom_types[0].members[0].types[0].name, "i32");
    }

    // ---- contractmetav0 (SEP-46 / SEP-58 provenance) ----
    //
    // Metadata is an open key/value space; these fixtures serialize real
    // `SCMetaEntry` XDR values (stellar-xdr), never hand-rolled bytes, and one
    // test runs against a REAL committed contract WASM carrying metadata.

    fn meta_entry(key: &str, val: &str) -> ScMetaEntry {
        ScMetaEntry::ScMetaV0(stellar_xdr::ScMetaV0 {
            key: key.to_string().try_into().unwrap(),
            val: val.to_string().try_into().unwrap(),
        })
    }

    /// Encodes `SCMetaEntry` values into a bare `contractmetav0` payload.
    fn encode_entries(entries: &[ScMetaEntry]) -> Vec<u8> {
        let mut payload = Vec::new();
        for e in entries {
            let mut buf = Vec::new();
            let mut cursor = Cursor::new(&mut buf);
            let mut l = Limited::new(&mut cursor, Limits::none());
            e.write_xdr(&mut l).unwrap();
            payload.extend_from_slice(&buf);
        }
        payload
    }

    /// Wraps a `contractmetav0` payload in a real WASM custom section.
    fn encode_meta_section(entries: &[ScMetaEntry]) -> Vec<u8> {
        let mut section = Vec::new();
        section.push(CONTRACT_META_V0.len() as u8);
        section.extend_from_slice(CONTRACT_META_V0.as_bytes());
        section.extend_from_slice(&encode_entries(entries));
        let mut result = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        result.push(0); // custom-section id
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
    fn decode_contract_meta_reads_generic_key_values_in_order() {
        let payload = vec![
            meta_entry("rsver", "1.97.1"),
            meta_entry("totally_unknown_key", "arbitrary value"),
            meta_entry("cliver", "27.1.0#deadbeef"),
        ];
        let decoded = decode_contract_meta(&encode_entries(&payload)).unwrap();
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].key, "rsver");
        assert_eq!(decoded[0].value, "1.97.1");
        assert_eq!(decoded[1].key, "totally_unknown_key");
        assert_eq!(decoded[1].value, "arbitrary value");
        assert_eq!(decoded[2].value, "27.1.0#deadbeef");
    }

    #[test]
    fn decode_contract_meta_preserves_duplicate_keys() {
        let payload = vec![
            meta_entry("k", "first"),
            meta_entry("k", "second"),
            meta_entry("k", "third"),
        ];
        let decoded = decode_contract_meta(&encode_entries(&payload)).unwrap();
        let values: Vec<&str> = decoded.iter().map(|e| e.value.as_str()).collect();
        assert_eq!(values, vec!["first", "second", "third"]);
    }

    #[test]
    fn decode_contract_meta_empty_payload_is_empty_list() {
        assert!(decode_contract_meta(&[]).unwrap().is_empty());
    }

    #[test]
    fn decode_contract_meta_rejects_malformed_payload() {
        // 0xFF is not a valid SCMetaKind discriminant.
        assert!(matches!(
            decode_contract_meta(&[0xff, 0x00, 0x00]).unwrap_err(),
            WasmError::MetaXdr(_)
        ));
        // A truncated string length must be rejected, not silently truncated.
        assert!(matches!(
            decode_contract_meta(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40]).unwrap_err(),
            WasmError::MetaXdr(_)
        ));
    }

    #[test]
    fn parse_metadata_surfaces_meta_and_keeps_custom_sections_unchanged() {
        let wasm = encode_meta_section(&[
            meta_entry("rsver", "1.97.1"),
            meta_entry("cliver", "27.1.0#8e402ea282029"),
        ]);
        let meta = crate::parse_metadata(&wasm).unwrap();
        // Existing behavior preserved: section still listed by name.
        assert_eq!(meta.custom_sections, vec![CONTRACT_META_V0.to_string()]);
        assert_eq!(meta.contract_meta.len(), 2);
        assert_eq!(meta.contract_meta[0].key, "rsver");
        assert_eq!(meta.contract_meta[1].value, "27.1.0#8e402ea282029");
    }

    #[test]
    fn parse_metadata_without_meta_section_is_empty() {
        let meta = crate::parse_metadata(VALID_WASM).unwrap();
        assert!(meta.contract_meta.is_empty());
        assert!(meta.custom_sections.is_empty());
    }

    /// A `contractspecv0` payload nesting `ScSpecTypeDef` past
    /// [`MAX_SPEC_XDR_DEPTH`] must surface as a decode error, not a SIGABRT.
    ///
    /// `ScSpecTypeDef::Vec` is XDR arm 1 and carries one nested typedef, so
    /// `depth` bytes of `0x00 0x00 0x00 0x01` build a chain that deep. With
    /// `Limits::none()` the recursive decoder walks the whole chain and dies
    /// on the Rust stack limit before returning.
    #[test]
    fn deeply_nested_spec_type_def_is_rejected_not_aborted() {
        let mut payload = Vec::new();
        let nesting = MAX_SPEC_XDR_DEPTH + 64;
        for _ in 0..nesting {
            payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        }
        let mut functions = Vec::new();
        let mut custom_types = Vec::new();
        let mut events = Vec::new();
        let err = decode_spec_section(&payload, &mut functions, &mut custom_types, &mut events)
            .expect_err("nesting past the depth cap must be a decode error");
        assert!(
            matches!(err, WasmError::SpecXdr(_)),
            "depth-limit failure must map to SpecXdr, got {err:?}"
        );
    }

    /// The `contractmetav0` decoder carries the same cap.
    ///
    /// `ScMetaEntry` is a flat union — one arm holding two XDR strings — so no
    /// payload can actually recurse past the depth cap; the limit is applied
    /// for consistency with the spec decoder and to bound `len`. What the cap
    /// *does* guarantee is that a string claiming more bytes than the section
    /// holds is rejected instead of being read out of bounds.
    #[test]
    fn meta_decoder_applies_bounded_limits() {
        // A length prefix of 0x40 (64 bytes) with no payload following.
        assert!(
            matches!(
                decode_contract_meta(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40])
                    .unwrap_err(),
                WasmError::MetaXdr(_)
            ),
            "a string longer than the section must be rejected"
        );
        // The cap must not reject well-formed entries.
        let entries = decode_contract_meta(&[
            0x00, 0x00, 0x00, 0x00, // ScMetaV0
            0x00, 0x00, 0x00, 0x00, // key: ""
            0x00, 0x00, 0x00, 0x00, // val: ""
        ])
        .expect("well-formed empty entry decodes");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "");
        assert_eq!(entries[0].value, "");
    }

    /// A spec section that nests within the cap must decode exactly as before
    /// — the limit must not reject legitimate contracts.
    #[test]
    fn shallow_spec_section_still_decodes() {
        let mut functions = Vec::new();
        let mut custom_types = Vec::new();
        let mut events = Vec::new();
        // Empty section is valid and yields nothing.
        decode_spec_section(&[], &mut functions, &mut custom_types, &mut events)
            .expect("empty spec section decodes");
        assert!(functions.is_empty() && custom_types.is_empty() && events.is_empty());
    }

    #[test]
    fn parse_metadata_decodes_real_committed_contract_metadata() {
        // `us_new.wasm` is a real compiled Soroban contract committed here; it
        // genuinely carries `contractmetav0`.
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../sdkt-cli/tests/fixtures/us_new.wasm"
        ))
        .expect("fixture present");
        let meta = crate::parse_metadata(&bytes).unwrap();
        assert!(meta.custom_sections.contains(&CONTRACT_META_V0.to_string()));
        assert!(
            !meta.contract_meta.is_empty(),
            "real contract metadata must decode"
        );
        let get = |k: &str| {
            meta.contract_meta
                .iter()
                .find(|e| e.key == k)
                .map(|e| e.value.clone())
        };
        assert_eq!(get("rsver").as_deref(), Some("1.97.1"));
        assert!(get("cliver").unwrap().starts_with("27.1.0#"));
    }

    // ---- contractenvmetav0 (SCEnvMetaEntry union) ----
    //
    // The payload is a 12-byte union: kind (u32) + protocol (u32) +
    // pre_release (u32). Fixtures serialize real `ScEnvMetaEntry` XDR values
    // via stellar-xdr; the tracked `us_new.wasm` fixture carries
    // kind=0, protocol=22, pre_release=0.

    /// Encodes an `ScEnvMetaEntry` into a bare `contractenvmetav0` payload.
    fn encode_env_meta(entry: &ScEnvMetaEntry) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        let mut l = Limited::new(&mut cursor, Limits::none());
        entry.write_xdr(&mut l).unwrap();
        buf
    }

    /// Wraps a `contractenvmetav0` payload in a real WASM custom section.
    fn encode_env_meta_section(payload: &[u8]) -> Vec<u8> {
        encode_custom_section(CONTRACT_ENV_META_V0, payload)
    }

    /// Encodes an arbitrary custom section (section id 0, uleb128 size) WITHOUT
    /// the WASM magic/version header — append to an existing module blob.
    fn encode_custom_section(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut section = Vec::new();
        section.push(name.len() as u8);
        section.extend_from_slice(name.as_bytes());
        section.extend_from_slice(payload);
        let mut result = Vec::new();
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

    fn env_meta_entry(protocol: u32, pre_release: u32) -> ScEnvMetaEntry {
        ScEnvMetaEntry::ScEnvMetaKindInterfaceVersion(stellar_xdr::ScEnvMetaEntryInterfaceVersion {
            protocol,
            pre_release,
        })
    }

    #[test]
    fn env_meta_decodes_protocol_and_pre_release() {
        let payload = encode_env_meta(&env_meta_entry(22, 0));
        assert_eq!(payload.len(), 12);
        // `spec_section` returns magic+version + the spec custom section. Append
        // the env-meta custom section directly after it (real WASM layout).
        let mut wasm = spec_section(&[func_entry("greet", vec![])]);
        wasm.extend_from_slice(&encode_env_meta_section(&payload));
        let spec = parse_contract_spec(&wasm).unwrap();
        assert_eq!(
            spec.env_meta,
            Some(EnvMetaSpec {
                protocol: 22,
                pre_release: 0,
            })
        );
    }

    #[test]
    fn env_meta_pre_release_difference_is_not_lost() {
        // Two payloads differing only in pre_release must decode differently.
        // Each WASM carries a minimal spec section so parse_contract_spec
        // accepts it (real WASM layout: spec + env-meta sections).
        let wasm_with = |pre_release: u32| {
            let payload = encode_env_meta(&env_meta_entry(22, pre_release));
            let mut wasm = spec_section(&[func_entry("greet", vec![])]);
            wasm.extend_from_slice(&encode_env_meta_section(&payload));
            wasm
        };
        let a = parse_contract_spec(&wasm_with(0)).unwrap();
        let b = parse_contract_spec(&wasm_with(5)).unwrap();
        assert_ne!(a.env_meta, b.env_meta);
        assert_eq!(
            b.env_meta,
            Some(EnvMetaSpec {
                protocol: 22,
                pre_release: 5,
            })
        );
    }

    #[test]
    fn env_meta_unknown_kind_is_rejected() {
        // kind = 1 is not a defined SCEnvMetaKind variant. The old decoder
        // folded the discriminant into a u64 (0x1_0000_0016); the union decode
        // must reject it instead.
        let mut payload = vec![0x00, 0x00, 0x00, 0x01]; // kind = 1
        payload.extend_from_slice(&22u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(payload.len(), 12);
        let err = decode_env_meta_section(&payload).expect_err("unknown kind must error");
        assert!(matches!(err, WasmError::MetaXdr(_)), "got {err:?}");
    }

    #[test]
    fn env_meta_truncated_payload_is_rejected() {
        // kind + protocol only (8 bytes): the old decoder accepted this and
        // silently dropped pre_release.
        let mut payload = vec![0x00, 0x00, 0x00, 0x00];
        payload.extend_from_slice(&22u32.to_be_bytes());
        assert_eq!(payload.len(), 8);
        let err = decode_env_meta_section(&payload).expect_err("truncated payload must error");
        assert!(matches!(err, WasmError::MetaXdr(_)), "got {err:?}");
        // A 12-byte payload must still decode.
        let mut full = payload;
        full.extend_from_slice(&0u32.to_be_bytes());
        decode_env_meta_section(&full).expect("12-byte payload decodes");
    }

    #[test]
    fn wasm_without_env_meta_section_yields_none() {
        let wasm = spec_section(&[func_entry("greet", vec![])]);
        let spec = parse_contract_spec(&wasm).unwrap();
        assert_eq!(spec.env_meta, None);
    }

    #[test]
    fn env_meta_decodes_real_committed_contract_fixture() {
        // `us_new.wasm` carries `contractenvmetav0` with kind=0, protocol=22,
        // pre_release=0 — the single tracked fixture required for
        // reproduction. Read the same way as the contractmetav0 fixture test.
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../sdkt-cli/tests/fixtures/us_new.wasm"
        ))
        .expect("fixture present");
        let spec = parse_contract_spec(&bytes).expect("fixture decodes");
        assert_eq!(
            spec.env_meta,
            Some(EnvMetaSpec {
                protocol: 22,
                pre_release: 0,
            })
        );
    }
}
