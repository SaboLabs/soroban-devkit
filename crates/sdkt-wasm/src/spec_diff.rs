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

/// Kind of ABI delta observed during an upgrade-safety check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    RemovedFunction,
    ChangedSignature,
    RemovedEvent,
    RemovedType,
    AddedFunction,
    AddedEvent,
    AddedType,
}

/// A single classified delta, used for both breaking and non-breaking lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictChange {
    pub kind: ChangeKind,
    /// Item name (function/event/type). Functions are stored bare; the
    /// human label adds `()` for call-style rendering.
    pub name: String,
    /// Optional human-readable detail (e.g. old/new signature for a change).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl VerdictChange {
    /// Human-readable `Kind: name()` label for pretty output.
    pub fn label(&self) -> String {
        let kind = match self.kind {
            ChangeKind::RemovedFunction => "Removed function",
            ChangeKind::ChangedSignature => "Changed signature",
            ChangeKind::RemovedEvent => "Removed event",
            ChangeKind::RemovedType => "Removed type",
            ChangeKind::AddedFunction => "Added function",
            ChangeKind::AddedEvent => "Added event",
            ChangeKind::AddedType => "Added type",
        };
        match self.kind {
            ChangeKind::RemovedFunction
            | ChangeKind::AddedFunction
            | ChangeKind::ChangedSignature => {
                format!("{}: {}()", kind, self.name)
            }
            _ => format!("{}: {}", kind, self.name),
        }
    }
}

/// An actionable upgrade-safety verdict derived from a [`SpecDiff`].
///
/// `compatible` is `false` when any *breaking* change is present (removed
/// function, changed signature, removed event, removed type). Additions are
/// non-breaking and recorded separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpgradeVerdict {
    pub compatible: bool,
    pub breaking_changes: Vec<VerdictChange>,
    pub non_breaking_changes: Vec<VerdictChange>,
}

impl UpgradeVerdict {
    /// Classify a [`SpecDiff`] into an upgrade-safety verdict.
    ///
    /// This deliberately reuses the existing diff output; no comparison logic
    /// is duplicated.
    pub fn from_diff(diff: &SpecDiff) -> Self {
        let mut breaking = Vec::new();
        let mut non_breaking = Vec::new();

        for f in &diff.removed_functions {
            breaking.push(VerdictChange {
                kind: ChangeKind::RemovedFunction,
                name: f.name.clone(),
                detail: String::new(),
            });
        }
        for c in &diff.changed_functions {
            breaking.push(VerdictChange {
                kind: ChangeKind::ChangedSignature,
                name: c.name.clone(),
                detail: format!("old: {}\n      new: {}", sig_of(&c.old), sig_of(&c.new)),
            });
        }
        for e in &diff.removed_events {
            breaking.push(VerdictChange {
                kind: ChangeKind::RemovedEvent,
                name: e.clone(),
                detail: String::new(),
            });
        }
        for t in &diff.removed_types {
            breaking.push(VerdictChange {
                kind: ChangeKind::RemovedType,
                name: t.clone(),
                detail: String::new(),
            });
        }
        for f in &diff.added_functions {
            non_breaking.push(VerdictChange {
                kind: ChangeKind::AddedFunction,
                name: f.name.clone(),
                detail: String::new(),
            });
        }
        for e in &diff.added_events {
            non_breaking.push(VerdictChange {
                kind: ChangeKind::AddedEvent,
                name: e.clone(),
                detail: String::new(),
            });
        }
        for t in &diff.added_types {
            non_breaking.push(VerdictChange {
                kind: ChangeKind::AddedType,
                name: t.clone(),
                detail: String::new(),
            });
        }

        let compatible = breaking.is_empty();
        Self {
            compatible,
            breaking_changes: breaking,
            non_breaking_changes: non_breaking,
        }
    }
}

/// Build a `name(params) -> outputs` signature string for a function.
fn sig_of(f: &ContractFunction) -> String {
    let params = f
        .parameters
        .iter()
        .map(|p| p.type_.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let outputs = f
        .outputs
        .iter()
        .map(|o| o.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}({}) -> ({})", f.name, params, outputs)
}

/// Compute an upgrade-safety verdict from two already-parsed [`ContractSpec`]s.
///
/// Reuses [`diff_specs`] (no duplicated comparison logic).
pub fn upgrade_safety(old: &ContractSpec, new: &ContractSpec) -> UpgradeVerdict {
    let diff =
        diff_specs(old, new, WasmSummary::default(), WasmSummary::default()).unwrap_or_default();
    UpgradeVerdict::from_diff(&diff)
}

/// Compute an upgrade-safety verdict directly from two raw WASM binaries.
pub fn upgrade_safety_wasm(old_raw: &[u8], new_raw: &[u8]) -> Result<UpgradeVerdict, WasmError> {
    let diff = diff_wasm(old_raw, new_raw)?;
    Ok(UpgradeVerdict::from_diff(&diff))
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

    // ---- Upgrade-safety verdict tests (reuse diff_specs, no new comparison) ----

    #[test]
    fn verdict_flags_removed_function_breaking() {
        let old = spec_section(&[func_entry("a", vec![]), func_entry("b", vec![])]);
        let new = spec_section(&[func_entry("a", vec![])]);
        let v = upgrade_safety_wasm(&old, &new).unwrap();
        assert!(!v.compatible);
        assert_eq!(v.breaking_changes.len(), 1);
        assert_eq!(v.breaking_changes[0].kind, ChangeKind::RemovedFunction);
        assert_eq!(v.breaking_changes[0].name, "b");
    }

    #[test]
    fn verdict_flags_changed_signature_breaking() {
        let old = spec_section(&[func_entry("f", vec![("x".into(), ScSpecTypeDef::U32)])]);
        let new = spec_section(&[func_entry("f", vec![("x".into(), ScSpecTypeDef::U64)])]);
        let v = upgrade_safety_wasm(&old, &new).unwrap();
        assert!(!v.compatible);
        assert_eq!(v.breaking_changes.len(), 1);
        assert_eq!(v.breaking_changes[0].kind, ChangeKind::ChangedSignature);
        assert_eq!(v.breaking_changes[0].name, "f");
    }

    #[test]
    fn verdict_flags_removed_event_breaking() {
        use crate::spec::tests::event_entry;
        let old = spec_section(&[event_entry("Transfer")]);
        let new = spec_section(&[]);
        let v = upgrade_safety_wasm(&old, &new).unwrap();
        assert!(!v.compatible);
        assert_eq!(v.breaking_changes.len(), 1);
        assert_eq!(v.breaking_changes[0].kind, ChangeKind::RemovedEvent);
        assert_eq!(v.breaking_changes[0].name, "Transfer");
    }

    #[test]
    fn verdict_flags_removed_type_breaking() {
        use crate::spec::tests::udt_struct_entry;
        let old = spec_section(&[udt_struct_entry("Point")]);
        let new = spec_section(&[]);
        let v = upgrade_safety_wasm(&old, &new).unwrap();
        assert!(!v.compatible);
        assert_eq!(v.breaking_changes.len(), 1);
        assert_eq!(v.breaking_changes[0].kind, ChangeKind::RemovedType);
        assert_eq!(v.breaking_changes[0].name, "Point");
    }

    #[test]
    fn verdict_only_additions_is_compatible() {
        let old = spec_section(&[func_entry("a", vec![])]);
        let new = spec_section(&[
            func_entry("a", vec![]),
            func_entry("b", vec![("x".into(), ScSpecTypeDef::U32)]),
        ]);
        use crate::spec::tests::event_entry;
        let old2 = spec_section(&[func_entry("a", vec![])]);
        let new2 = spec_section(&[func_entry("a", vec![]), event_entry("Mint")]);
        let _ = (old, new); // silence unused
        let v = upgrade_safety_wasm(&old2, &new2).unwrap();
        assert!(v.compatible);
        assert!(v.breaking_changes.is_empty());
        assert!(!v.non_breaking_changes.is_empty());
        assert!(v
            .non_breaking_changes
            .iter()
            .any(|c| c.kind == ChangeKind::AddedEvent && c.name == "Mint"));
    }

    #[test]
    fn verdict_identical_is_compatible() {
        let wasm = spec_section(&[func_entry(
            "greet",
            vec![("name".into(), ScSpecTypeDef::String)],
        )]);
        let v = upgrade_safety_wasm(&wasm, &wasm).unwrap();
        assert!(v.compatible);
        assert!(v.breaking_changes.is_empty());
        assert!(v.non_breaking_changes.is_empty());
    }
}
