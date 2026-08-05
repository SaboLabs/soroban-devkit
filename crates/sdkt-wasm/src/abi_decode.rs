//! ABI-aware ScVal decoding helpers for ENG-16.

use crate::spec::{ContractEvent, ContractType};
use stellar_xdr::ScVal;

/// Human-readable label for a ScVal given ABI context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedValue {
    /// The raw ScVal representation.
    pub raw: String,
    /// ABI-derived type name, if found.
    pub abi_type: Option<String>,
    /// A shorter human-readable description.
    pub label: String,
}

/// Look up event ABI info by name.
pub fn find_event_abi<'a>(
    events: &'a [ContractEvent],
    event_name: &'a str,
) -> Option<&'a ContractEvent> {
    events.iter().find(|e| e.name == event_name)
}

/// Look up a custom type by name.
pub fn find_type_abi<'a>(
    types: &'a [ContractType],
    type_name: &'a str,
) -> Option<&'a ContractType> {
    types.iter().find(|t| t.name == type_name)
}

/// Format an ScVal into a human-readable string using ABI hints.
pub fn format_scval_abi(
    val: &ScVal,
    event_abi: Option<&[ContractEvent]>,
    _types_abi: Option<&[ContractType]>,
    event_hint: Option<&str>,
) -> DecodedValue {
    let raw_repr = format!("{:?}", val);
    let raw_str = raw_repr.clone();

    let label = match (event_hint, event_abi) {
        (Some(name), Some(events)) => {
            if find_event_abi(events, name).is_some() {
                format!("event[{}] -> value", name)
            } else {
                raw_repr
            }
        }
        _ => raw_repr,
    };

    DecodedValue {
        raw: raw_str,
        abi_type: None,
        label,
    }
}

#[cfg(test)]
mod abi_tests {
    use super::*;
    use crate::spec::ContractEvent;
    use stellar_xdr::ScVal;

    fn dummy_events() -> Vec<ContractEvent> {
        vec![ContractEvent {
            name: "transfer".to_string(),
            doc: "transfer event".to_string(),
        }]
    }

    #[test]
    fn format_scval_abi_with_event_hint() {
        let events = dummy_events();
        let val = ScVal::U32(42);
        let result = format_scval_abi(&val, Some(&events), None, Some("transfer"));
        assert!(result.label.contains("event[transfer]"));
    }

    #[test]
    fn find_event_abi_exists() {
        let events = dummy_events();
        assert!(find_event_abi(&events, "transfer").is_some());
        assert!(find_event_abi(&events, "unknown").is_none());
    }
}
