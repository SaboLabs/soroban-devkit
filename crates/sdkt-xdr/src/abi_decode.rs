//! ABI-aware ScVal decoding using ContractSpec (ENG-16).
//!
//! This module provides the core decoding logic that maps raw `ScVal` values
//! to human-readable representations using the contract's ABI (ContractSpec).

use sdkt_wasm::spec::ContractSpec;
use stellar_xdr::ScVal;

/// Result of ABI-aware decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiDecodedValue {
    /// Raw ScVal debug representation.
    pub raw: String,
    /// Human-readable label (e.g., "event[transfer] -> from: G..., to: G..., amount: 100").
    pub label: String,
    /// If matched to a specific ContractType/Event.
    pub matched_type: Option<String>,
    /// Structured key-value fields if decodable as a compound type.
    pub fields: Option<Vec<(String, String)>>,
}

/// Decode a single ScVal using the contract's ABI.
///
/// - `spec`: Parsed ContractSpec from the contract's WASM.
/// - `val`: The ScVal to decode.
/// - `event_hint`: Optional event name if this value came from an event (topics[0]).
pub fn decode_with_abi(
    spec: &ContractSpec,
    val: &ScVal,
    event_hint: Option<&str>,
) -> AbiDecodedValue {
    let raw = format!("{:?}", val);

    // Try event-based decoding first
    if let Some(event_name) = event_hint {
        if spec.events.iter().any(|e| e.name == event_name) {
            // Event found in ABI - we can provide better labeling
            return AbiDecodedValue {
                raw: raw.clone(),
                label: format!(
                    "event[{}] -> {}",
                    event_name,
                    decode_scval_to_string(val, spec)
                ),
                matched_type: Some(format!("event:{}", event_name)),
                fields: extract_scval_fields(val, spec),
            };
        }
    }

    // Try to match against custom types (UDTs)
    for custom_type in &spec.custom_types {
        if let Some(fields) = extract_scval_fields(val, spec) {
            if !fields.is_empty() {
                return AbiDecodedValue {
                    raw: raw.clone(),
                    label: format!(
                        "{} {{ {} }}",
                        custom_type.name,
                        fields
                            .iter()
                            .map(|(k, v)| format!("{}: {}", k, v))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    matched_type: Some(format!("udt:{}", custom_type.name)),
                    fields: Some(fields),
                };
            }
        }
    }

    // Fallback: basic ScVal string representation
    AbiDecodedValue {
        raw: raw.clone(),
        label: decode_scval_to_string(val, spec),
        matched_type: None,
        fields: extract_scval_fields(val, spec),
    }
}

/// Convert ScVal to a human-readable string using basic type info.
fn decode_scval_to_string(val: &ScVal, _spec: &ContractSpec) -> String {
    match val {
        ScVal::Bool(b) => format!("bool({})", b),
        ScVal::U32(n) => format!("u32({})", n),
        ScVal::I32(n) => format!("i32({})", n),
        ScVal::U64(n) => format!("u64({})", n),
        ScVal::I64(n) => format!("i64({})", n),
        ScVal::U128(p) => format!(
            "u128({} hi={}, lo={})",
            ((p.hi as u128) << 64) | p.lo as u128,
            p.hi,
            p.lo
        ),
        ScVal::I128(p) => format!(
            "i128({} hi={}, lo={})",
            ((p.hi as i128) << 64) | (p.lo as i128),
            p.hi,
            p.lo
        ),
        ScVal::U256(_) => "u256(...)".to_string(),
        ScVal::I256(_) => "i256(...)".to_string(),
        ScVal::String(s) => format!("\"{}\"", s.to_utf8_string_lossy()),
        ScVal::Symbol(s) => format!("sym(\"{}\")", s.to_utf8_string_lossy()),
        ScVal::Bytes(b) => format!("bytes({} bytes)", b.len()),
        ScVal::Vec(v) => {
            let len = v.as_ref().map(|items| items.len()).unwrap_or(0);
            format!("vec(len={})", len)
        }
        // Note: MuxedAddress, Option, Result, Timepoint, Duration, Error are not direct ScVal variants in stellar-xdr 28; omitted for minimal ENG-16 scope.
        _ => format!("scval({:?})", val),
    }
}

/// Extract key-value fields from a compound ScVal (Vec of pairs, Map, Struct).
fn extract_scval_fields(val: &ScVal, _spec: &ContractSpec) -> Option<Vec<(String, String)>> {
    match val {
        ScVal::Vec(v) => {
            let mut fields = Vec::new();
            if let Some(items) = v.as_ref() {
                for item in items.iter() {
                    if let ScVal::Vec(pair) = item {
                        if let Some(pair_items) = pair.as_ref() {
                            if pair_items.len() == 2 {
                                let key = decode_scval_to_string(&pair_items[0], _spec);
                                let value = decode_scval_to_string(&pair_items[1], _spec);
                                fields.push((key, value));
                            }
                        }
                    }
                }
            }
            if fields.is_empty() {
                None
            } else {
                Some(fields)
            }
        }
        ScVal::Map(m) => {
            let mut fields = Vec::new();
            if let Some(items) = m.as_ref() {
                for entry in items.iter() {
                    let key = decode_scval_to_string(&entry.key, _spec);
                    let value = decode_scval_to_string(&entry.val, _spec);
                    fields.push((key, value));
                }
            }
            if fields.is_empty() {
                None
            } else {
                Some(fields)
            }
        }
        _ => None,
    }
}

/// Decode multiple ScVals (e.g., event topics + data).
pub fn decode_event_topics(
    spec: &ContractSpec,
    topics: &[ScVal],
    data: &[ScVal],
) -> Vec<AbiDecodedValue> {
    let mut results = Vec::new();

    // Topic 0 is typically the event name (Symbol)
    let event_name = topics.first().and_then(|t| match t {
        ScVal::Symbol(s) => Some(s.to_utf8_string_lossy()),
        _ => None,
    });

    // Decode topics with event hint
    for (i, topic) in topics.iter().enumerate() {
        let hint = if i == 0 { None } else { event_name.as_deref() };
        results.push(decode_with_abi(spec, topic, hint));
    }

    // Decode data values
    for val in data {
        results.push(decode_with_abi(spec, val, event_name.as_deref()));
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkt_wasm::spec::ContractEvent;
    use stellar_xdr::{ScSymbol, ScVal};

    fn dummy_spec() -> ContractSpec {
        ContractSpec {
            env_meta: None,
            functions: vec![],
            custom_types: vec![],
            events: vec![ContractEvent {
                name: "transfer".to_string(),
                doc: "Transfer event".to_string(),
            }],
        }
    }

    #[test]
    fn decode_simple_primitives() {
        let spec = dummy_spec();
        let val = ScVal::U32(42);
        let decoded = decode_with_abi(&spec, &val, None);
        assert!(decoded.label.contains("u32(42)"));
    }

    #[test]
    fn decode_string() {
        use stellar_xdr::ScString;
        let spec = dummy_spec();
        let val = ScVal::String(ScString(
            "hello".to_string().into_bytes().try_into().unwrap(),
        ));
        let decoded = decode_with_abi(&spec, &val, None);
        assert!(decoded.label.contains("hello"));
    }

    #[test]
    fn decode_event_with_hint() {
        let spec = dummy_spec();
        let val = ScVal::U64(100);
        let decoded = decode_with_abi(&spec, &val, Some("transfer"));
        assert!(decoded.label.contains("event[transfer]"));
        assert_eq!(decoded.matched_type, Some("event:transfer".to_string()));
    }

    #[test]
    fn decode_event_topics_single() {
        let spec = dummy_spec();
        let topics = vec![ScVal::Symbol(ScSymbol("transfer".try_into().unwrap()))];
        let data = vec![ScVal::U64(100)];
        let decoded = decode_event_topics(&spec, &topics, &data);
        assert_eq!(decoded.len(), 2);
        assert!(decoded[0].label.contains("sym"));
        assert!(decoded[1].label.contains("event[transfer]"));
    }

    #[test]
    fn abi_decodes_primitive_with_hint() {
        let spec = dummy_spec();
        let val = ScVal::U32(42);
        let decoded = decode_with_abi(&spec, &val, Some("transfer"));
        assert!(decoded.label.contains("event[transfer]"));
        assert_eq!(decoded.matched_type.as_deref(), Some("event:transfer"));
        assert!(decoded.fields.is_none());
    }
}
