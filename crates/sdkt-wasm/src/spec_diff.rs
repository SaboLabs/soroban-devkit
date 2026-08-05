//! Offline contract comparison (ABI / WASM diff).
//!
//! Compares two compiled Soroban WASM binaries by extracting their
//! `ContractSpec` (functions, events, custom types) via the existing
//! [`crate::parse_contract_spec`] parser, then classifying the deltas.
//!
//! Everything is offline: no RPC, no network. The diff operates purely on the
//! declared ABI, so it is a safe pre-upgrade check (detect breaking changes
//! before `sdkt deploy` of a new WASM).

use serde::{Deserialize, Serialize};

use crate::{parse_contract_spec, parse_metadata, ContractFunction, ContractSpec, WasmError};

/// The full comparison result between two contract WASM binaries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SpecDiff {
    /// Metadata of the "old" (baseline) WASM.
    pub old: WasmSummary,
    /// Metadata of the "new" (candidate) WASM.
    pub new: WasmSummary,
    /// Functions present in `new` but absent from `old`.
    pub added_functions: Vec<ContractFunction>,
    /// Functions present in `old` but absent from `new`.
    pub removed_functions: Vec<ContractFunction>,
    /// Functions present in both whose signature (inputs + outputs) changed.
    pub changed_functions: Vec<FunctionSignatureChange>,
    /// Events present in `new` but absent from `old`.
    pub added_events: Vec<String>,
    /// Events present in `old` but absent from `new`.
    pub removed_events: Vec<String>,
    /// Custom types present in `new` but absent from `old`.
    pub added_types: Vec<String>,
    /// Custom types present in `old` but absent from `new`.
    pub removed_types: Vec<String>,
}

/// Lightweight WASM identity summary for diff context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WasmSummary {
    /// SHA-256 hex of the raw WASM bytes.
    pub hash: String,
    /// Size in bytes.
    pub size_bytes: usize,
}

/// A function whose signature changed between old and new.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionSignatureChange {
    pub name: String,
    pub old: ContractFunction,
    pub new: ContractFunction,
}

impl SpecDiff {
    /// True when the two WASM binaries declare an identical ABI surface
    /// (same functions, signatures, events, and custom types).
    pub fn is_identical(&self) -> bool {
        self.added_functions.is_empty()
            && self.removed_functions.is_empty()
            && self.changed_functions.is_empty()
            && self.added_events.is_empty()
            && self.removed_events.is_empty()
            && self.added_types.is_empty()
            && self.removed_types.is_empty()
    }

    /// Total number of breaking/non-breaking deltas (a single count used for
    /// quick pass/fail triage in CI).
    pub fn total_changes(&self) -> usize {
        self.added_functions.len()
            + self.removed_functions.len()
            + self.changed_functions.len()
            + self.added_events.len()
            + self.removed_events.len()
            + self.added_types.len()
            + self.removed_types.len()
    }
}

/// Convenience: build a [`WasmSummary`] from raw WASM bytes.
fn summarize(raw: &[u8]) -> Result<WasmSummary, WasmError> {
    let meta = parse_metadata(raw)?;
    Ok(WasmSummary {
        hash: meta.hash,
        size_bytes: meta.size_bytes,
    })
}

/// Diff two raw WASM binaries (offline).
///
/// Each side is parsed independently; if either fails to yield a
/// `ContractSpec`, the error is returned. Missing entries are treated as
/// empty lists, so a contract with no events still diffs cleanly against one
/// that declares events.
pub fn diff_wasm(old_raw: &[u8], new_raw: &[u8]) -> Result<SpecDiff, WasmError> {
    let old_spec = parse_contract_spec(old_raw)?;
    let new_spec = parse_contract_spec(new_raw)?;
    diff_specs(
        &old_spec,
        &new_spec,
        summarize(old_raw)?,
        summarize(new_raw)?,
    )
}

/// Diff two already-parsed [`ContractSpec`] values.
///
/// `old_summary` / `new_summary` carry WASM identity (hash/size) for context in
/// reports; pass `WasmSummary::default()` when not available.
pub fn diff_specs(
    old: &ContractSpec,
    new: &ContractSpec,
    old_summary: WasmSummary,
    new_summary: WasmSummary,
) -> Result<SpecDiff, WasmError> {
    let mut diff = SpecDiff {
        old: old_summary,
        new: new_summary,
        ..Default::default()
    };

    // Index old by name for O(n) lookups.
    let old_fns: std::collections::BTreeMap<&str, &ContractFunction> =
        old.functions.iter().map(|f| (f.name.as_str(), f)).collect();
    let new_fns: std::collections::BTreeMap<&str, &ContractFunction> =
        new.functions.iter().map(|f| (f.name.as_str(), f)).collect();

    for (name, nf) in &new_fns {
        match old_fns.get(name) {
            None => diff.added_functions.push((**nf).clone()),
            Some(of) => {
                if of.parameters != nf.parameters || of.outputs != nf.outputs {
                    diff.changed_functions.push(FunctionSignatureChange {
                        name: (*name).to_string(),
                        old: (*of).clone(),
                        new: (**nf).clone(),
                    });
                }
            }
        }
    }
    for (_name, of) in &old_fns {
        if !new_fns.contains_key(_name) {
            diff.removed_functions.push((**of).clone());
        }
    }

    diff_set(
        &old.events
            .iter()
            .map(|e| e.name.clone())
            .collect::<Vec<_>>(),
        &new.events
            .iter()
            .map(|e| e.name.clone())
            .collect::<Vec<_>>(),
        &mut diff.added_events,
        &mut diff.removed_events,
    );
    diff_set(
        &old.custom_types
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>(),
        &new.custom_types
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>(),
        &mut diff.added_types,
        &mut diff.removed_types,
    );

    Ok(diff)
}

/// Classify named items present in `new` but not `old` (`added`) and `old` but
/// not `new` (`removed`). Names are the identity key.
fn diff_set(old: &[String], new: &[String], added: &mut Vec<String>, removed: &mut Vec<String>) {
    let old_set: std::collections::BTreeSet<&str> = old.iter().map(String::as_str).collect();
    let new_set: std::collections::BTreeSet<&str> = new.iter().map(String::as_str).collect();

    for n in &new_set {
        if !old_set.contains(n) {
            added.push((*n).to_string());
        }
    }
    for n in &old_set {
        if !new_set.contains(n) {
            removed.push((*n).to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::tests::func_entry;
    use crate::spec::tests::spec_section;
    use stellar_xdr::ScSpecTypeDef;

    // Re-create small WASM blobs with the helper from spec.rs tests.

    #[test]
    fn identical_specs_diff_empty() {
        let wasm = spec_section(&[func_entry(
            "greet",
            vec![("name".into(), ScSpecTypeDef::String)],
        )]);
        let d = diff_wasm(&wasm, &wasm).unwrap();
        assert!(d.is_identical());
        assert_eq!(d.total_changes(), 0);
    }

    #[test]
    fn detects_added_function() {
        let old = spec_section(&[func_entry("a", vec![])]);
        let new = spec_section(&[
            func_entry("a", vec![]),
            func_entry("b", vec![("x".into(), ScSpecTypeDef::U32)]),
        ]);
        let d = diff_wasm(&old, &new).unwrap();
        assert_eq!(d.added_functions.len(), 1);
        assert_eq!(d.added_functions[0].name, "b");
        assert!(d.removed_functions.is_empty());
        assert!(d.changed_functions.is_empty());
    }

    #[test]
    fn detects_removed_function() {
        let old = spec_section(&[func_entry("a", vec![]), func_entry("b", vec![])]);
        let new = spec_section(&[func_entry("a", vec![])]);
        let d = diff_wasm(&old, &new).unwrap();
        assert_eq!(d.removed_functions.len(), 1);
        assert_eq!(d.removed_functions[0].name, "b");
    }

    #[test]
    fn detects_changed_signature() {
        let old = spec_section(&[func_entry("f", vec![("x".into(), ScSpecTypeDef::U32)])]);
        let new = spec_section(&[func_entry("f", vec![("x".into(), ScSpecTypeDef::U64)])]);
        let d = diff_wasm(&old, &new).unwrap();
        assert_eq!(d.changed_functions.len(), 1);
        assert_eq!(d.changed_functions[0].name, "f");
        assert_eq!(d.changed_functions[0].old.parameters[0].type_.name, "u32");
        assert_eq!(d.changed_functions[0].new.parameters[0].type_.name, "u64");
    }

    #[test]
    fn detects_added_removed_events() {
        use crate::spec::tests::event_entry;
        let old = spec_section(&[event_entry("Transfer")]);
        let new = spec_section(&[event_entry("Mint")]);
        let d = diff_wasm(&old, &new).unwrap();
        assert_eq!(d.added_events, vec!["Mint".to_string()]);
        assert_eq!(d.removed_events, vec!["Transfer".to_string()]);
    }

    #[test]
    fn detects_added_removed_types() {
        use crate::spec::tests::udt_struct_entry;
        let old = spec_section(&[udt_struct_entry("Point")]);
        let new = spec_section(&[udt_struct_entry("Circle")]);
        let d = diff_wasm(&old, &new).unwrap();
        assert_eq!(d.added_types, vec!["Circle".to_string()]);
        assert_eq!(d.removed_types, vec!["Point".to_string()]);
    }

    #[test]
    fn parse_error_propagates() {
        // "old" is valid but has no contract spec; "new" is a valid spec.
        let old = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let new = spec_section(&[func_entry("a", vec![])]);
        let err = diff_wasm(&old, &new);
        assert!(matches!(err, Err(WasmError::NoContractSpec)));
    }
}
