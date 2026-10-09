//! Storage snapshots, value-aware diffing and TTL extension-plan derivation.
//!
//! # Overview
//!
//! A [`StorageSnapshot`] records, for each tracked ledger key, its storage
//! class, durability, remaining TTL, estimated rent and — when the value can be
//! read — the captured value as a base64 XDR `ScVal`. Snapshots are plain JSON,
//! so a document written before value capture existed still deserializes with
//! `value: None` and diffs on TTL/rent only.
//!
//! [`diff_snapshots`] compares an **old** (baseline) snapshot against a **new**
//! (current) one and returns a [`SnapshotDiff`] carrying two views of the same
//! comparison:
//!
//! * the remediation view — one [`DiffEntry`] per key, classified as
//!   [`DiffStatus::Removed`], [`DiffStatus::ExpiringSoon`],
//!   [`DiffStatus::ValueChanged`] or [`DiffStatus::Unchanged`].  This is what
//!   [`derive_extend_plan`] consumes to build a non-mutating [`ExtendPlan`].
//! * the value-aware change sets — [`ValueDelta`] (captured value moved),
//!   [`TtlDelta`] (TTL/rent moved), plus `added`/`removed` keys.
//!
//! # Guarantees
//! - No RPC mutation methods are called in this module.
//! - An empty diff (no removed or expiring entries) produces an [`ExtendPlan`]
//!   with an empty key list and exits cleanly.
//!
//! # Tracked key set
//! Soroban RPC cannot enumerate a contract's storage: `getLedgerEntries` only
//! returns the keys that were explicitly requested. A live capture therefore
//! requests exactly the baseline snapshot's keys plus the contract instance
//! entry (and any extra keys the caller passes), so a key **created after** the
//! snapshot is invisible unless the caller supplies it explicitly.

use crate::analyzer::classify_key;
use crate::error::StorageError;
use crate::types::{StorageClass, StorageReport};
use base64::Engine;
use sdkt_rpc::SorobanRpcClient;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use stellar_xdr::WriteXdr;

/// Threshold in ledgers below which a live entry is flagged as "expiring soon".
/// ~1 day at 5 s/ledger (17 280 ledgers).  Matches the constant used in
/// [`crate::analyzer`] so the diff is consistent with the storage analyzer.
pub const EXPIRING_SOON_LEDGERS: u32 = 17_280;

/// Default suggested ledger horizon used when deriving an extend plan and the
/// remaining TTL gives no useful signal.  Equivalent to ~30 days at 5 s/ledger.
pub const DEFAULT_SUGGESTED_LEDGERS: u32 = 518_400;

// ---------------------------------------------------------------------------
// StorageSnapshot
// ---------------------------------------------------------------------------

/// A point-in-time capture of one contract's storage entries.
///
/// Callers may build it from a [`StorageReport`] via
/// [`StorageSnapshot::from_report`], from raw entries via
/// [`StorageSnapshot::from_entries`], or capture one directly over RPC with
/// [`capture_snapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StorageSnapshot {
    /// The on-chain contract identifier (C… StrKey or hex).
    pub contract_id: String,
    /// Ledger sequence at capture time, when it could be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at_ledger: Option<u32>,
    /// Entries captured at the time this snapshot was taken.
    #[serde(default)]
    pub entries: Vec<SnapshotEntry>,
}

/// A single entry inside a [`StorageSnapshot`].
///
/// Every field except `key` is defaulted so a document written before value
/// capture (or before class/durability were recorded) still parses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SnapshotEntry {
    /// Base64 XDR encoded `LedgerKey`.
    pub key: String,
    /// Readable ABI-derived key label, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Storage class derived from the ledger key.
    #[serde(default)]
    pub class: StorageClass,
    /// Durability of the entry (`persistent` / `temporary`); absent for
    /// non-contract-data keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability: Option<String>,
    /// TTL in ledgers relative to the ledger at snapshot time.
    #[serde(default)]
    pub current_ttl: u32,
    /// Estimated rent extension cost in stroops.
    #[serde(default)]
    pub extension_cost_stroops: u64,
    /// Captured entry value as a base64 XDR `ScVal`.
    ///
    /// Optional so snapshots written before value capture existed keep parsing;
    /// serialized only when a value was actually captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl StorageSnapshot {
    /// Construct a snapshot from a [`StorageReport`] produced by the analyzer.
    ///
    /// Returns an error if `report.total_entries > 0` but `report.entries` is empty,
    /// indicating a legacy or summary-only report without per-entry detail.
    pub fn from_report(report: &StorageReport) -> Result<Self, StorageError> {
        if report.total_entries > 0 && report.entries.is_empty() {
            return Err(StorageError::Parse(format!(
                "Snapshot report for contract '{}' has total_entries ({}) > 0 but entries array is empty (legacy or summary-only report without per-entry detail)",
                report.contract_id, report.total_entries
            )));
        }
        Ok(Self {
            contract_id: report.contract_id.clone(),
            captured_at_ledger: None,
            entries: report
                .entries
                .iter()
                .map(|e| SnapshotEntry {
                    key: e.key.clone(),
                    label: e.label.clone(),
                    class: e.class,
                    durability: durability_label(e.class),
                    current_ttl: e.current_ttl,
                    extension_cost_stroops: e.extension_cost_stroops,
                    value: None,
                })
                .collect(),
        })
    }

    /// Build a snapshot from raw `(key, ttl)` pairs (useful in tests and CLIs
    /// that already have the data from RPC without going through the analyzer).
    pub fn from_entries(contract_id: impl Into<String>, entries: Vec<SnapshotEntry>) -> Self {
        Self {
            contract_id: contract_id.into(),
            captured_at_ledger: None,
            entries,
        }
    }
}

// ---------------------------------------------------------------------------
// Value-aware change sets
// ---------------------------------------------------------------------------

/// A tracked entry whose captured value changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueDelta {
    /// Base64 XDR encoded `LedgerKey`.
    pub key: String,
    /// Storage class derived from the ledger key.
    pub class: StorageClass,
    /// Value in the base snapshot (base64 XDR `ScVal`).
    pub before: Option<String>,
    /// Value in the live snapshot (base64 XDR `ScVal`).
    pub after: Option<String>,
}

/// A tracked entry whose TTL or estimated rent changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TtlDelta {
    /// Base64 XDR encoded `LedgerKey`.
    pub key: String,
    /// Storage class derived from the ledger key.
    pub class: StorageClass,
    /// TTL in the base snapshot.
    pub before_ttl: u32,
    /// TTL in the live snapshot.
    pub after_ttl: u32,
    /// Estimated rent in the base snapshot, in stroops.
    pub before_extension_cost_stroops: u64,
    /// Estimated rent in the live snapshot, in stroops.
    pub after_extension_cost_stroops: u64,
}

// ---------------------------------------------------------------------------
// Diff types
// ---------------------------------------------------------------------------

/// Classification of a single entry in a snapshot diff.
///
/// This is the *remediation* view: it describes what the TTL extension plan
/// needs to know. A captured value change is reported separately in
/// [`SnapshotDiff::value_changed`] (and in `status` only when nothing more
/// urgent applies).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiffStatus {
    /// Entry was present in the old snapshot but is absent in the new one.
    /// This means it expired or was otherwise removed from the ledger.
    Removed,
    /// Entry is still live in the new snapshot but its TTL is below
    /// [`EXPIRING_SOON_LEDGERS`].  Action is recommended before it expires.
    ExpiringSoon,
    /// Entry is still live with a healthy TTL, but its captured value moved.
    ValueChanged,
    /// Entry is still live and its TTL is above the expiring-soon threshold.
    #[default]
    Unchanged,
}

/// A single entry in a [`SnapshotDiff`]'s remediation view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DiffEntry {
    /// Base64 XDR encoded `LedgerKey`.
    pub key: String,
    /// Readable ABI-derived key label, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How this entry changed between the two snapshots.
    pub status: DiffStatus,
    /// TTL in the old snapshot (`None` for entries that are `Removed` and were
    /// not observed at the new point in time).
    pub old_ttl: Option<u32>,
    /// TTL in the new snapshot (`None` for `Removed` entries).
    pub new_ttl: Option<u32>,
}

/// The complete result of diffing two [`StorageSnapshot`]s.
///
/// Carries both the per-entry remediation view (`entries`) and the value-aware
/// change sets (`value_changed`, `ttl_changed`, `added`, `removed`). The empty
/// change sets are omitted from JSON so a clean diff never advertises sections
/// that did not fire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SnapshotDiff {
    /// Contract these snapshots describe.
    pub contract_id: String,
    /// Remediation view: every tracked key across both snapshots.
    pub entries: Vec<DiffEntry>,
    /// Tracked entries present in both snapshots with identical value, TTL and
    /// rent. (Entries that only appear in the live capture are `added`, not
    /// unchanged.)
    pub unchanged: usize,
    /// Entries whose captured value changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub value_changed: Vec<ValueDelta>,
    /// Entries whose TTL and/or estimated rent changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ttl_changed: Vec<TtlDelta>,
    /// Keys present in the live snapshot but not in the base snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// Keys present in the base snapshot but not in the live snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

impl SnapshotDiff {
    /// Total number of deltas across all change kinds.
    pub fn changed(&self) -> usize {
        self.value_changed.len() + self.ttl_changed.len() + self.added.len() + self.removed.len()
    }

    /// Return only the entries that need remediation (removed or expiring soon).
    pub fn actionable(&self) -> impl Iterator<Item = &DiffEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e.status, DiffStatus::Removed | DiffStatus::ExpiringSoon))
    }
}

// ---------------------------------------------------------------------------
// diff_snapshots
// ---------------------------------------------------------------------------

/// Diff an **old** (baseline) snapshot against a **new** (current) one.
///
/// Entries are keyed by their canonical base64 XDR `LedgerKey`, so a key
/// expressed as hex in one document and base64 in the other still compares
/// equal. For each key present in both snapshots the value and the TTL/rent are
/// compared independently: a value-only change produces a [`ValueDelta`], a
/// TTL/rent-only change produces a [`TtlDelta`], and an entry where both match
/// counts as unchanged. A value is only compared when **both** sides captured
/// one, so snapshots written before value capture existed keep their original
/// TTL/rent-only diff semantics.
///
/// The remediation classification in [`DiffStatus`] is derived as follows:
/// 1. absent in the new snapshot → [`DiffStatus::Removed`];
/// 2. live with `new_ttl < EXPIRING_SOON_LEDGERS` → [`DiffStatus::ExpiringSoon`];
/// 3. live, healthy TTL, but captured value moved → [`DiffStatus::ValueChanged`];
/// 4. otherwise → [`DiffStatus::Unchanged`].
///
/// Keys that appear only in the new snapshot are reported as `added` (and, for
/// the remediation view, as `Unchanged` or `ExpiringSoon` depending on TTL).
pub fn diff_snapshots(
    old: &StorageSnapshot,
    new: &StorageSnapshot,
) -> Result<SnapshotDiff, StorageError> {
    if old.contract_id != new.contract_id {
        return Err(StorageError::ContractIdMismatch {
            old: old.contract_id.clone(),
            new: new.contract_id.clone(),
        });
    }

    let old_entries = index_entries(&old.entries);
    let new_entries = index_entries(&new.entries);

    let mut diff = SnapshotDiff {
        contract_id: old.contract_id.clone(),
        ..Default::default()
    };

    for (key, before) in &old_entries {
        let Some(after) = new_entries.get(key) else {
            diff.removed.push(key.clone());
            diff.entries.push(DiffEntry {
                key: key.clone(),
                label: before.label.clone(),
                status: DiffStatus::Removed,
                old_ttl: Some(before.current_ttl),
                new_ttl: None,
            });
            continue;
        };

        let value_moved = match (&before.value, &after.value) {
            (Some(before_value), Some(after_value)) => before_value != after_value,
            // Value capture was absent on one side: nothing to compare.
            _ => false,
        };
        let ttl_moved = before.current_ttl != after.current_ttl
            || before.extension_cost_stroops != after.extension_cost_stroops;

        if value_moved {
            diff.value_changed.push(ValueDelta {
                key: key.clone(),
                class: after.class,
                before: before.value.clone(),
                after: after.value.clone(),
            });
        }
        if ttl_moved {
            diff.ttl_changed.push(TtlDelta {
                key: key.clone(),
                class: after.class,
                before_ttl: before.current_ttl,
                after_ttl: after.current_ttl,
                before_extension_cost_stroops: before.extension_cost_stroops,
                after_extension_cost_stroops: after.extension_cost_stroops,
            });
        }
        if !value_moved && !ttl_moved {
            diff.unchanged += 1;
        }

        let status = if after.current_ttl < EXPIRING_SOON_LEDGERS {
            DiffStatus::ExpiringSoon
        } else if value_moved {
            DiffStatus::ValueChanged
        } else {
            DiffStatus::Unchanged
        };
        diff.entries.push(DiffEntry {
            key: key.clone(),
            label: after.label.clone().or_else(|| before.label.clone()),
            status,
            old_ttl: Some(before.current_ttl),
            new_ttl: Some(after.current_ttl),
        });
    }

    // Entries that are new (only present in the live snapshot).
    for (key, after) in &new_entries {
        if old_entries.contains_key(key) {
            continue;
        }
        diff.added.push(key.clone());
        let status = if after.current_ttl < EXPIRING_SOON_LEDGERS {
            DiffStatus::ExpiringSoon
        } else {
            DiffStatus::Unchanged
        };
        diff.entries.push(DiffEntry {
            key: key.clone(),
            label: after.label.clone(),
            status,
            old_ttl: None,
            new_ttl: Some(after.current_ttl),
        });
    }

    // Stable sort so output is deterministic (by key, then by status).
    diff.entries.sort_by(|a, b| a.key.cmp(&b.key));
    diff.value_changed.sort_by(|a, b| a.key.cmp(&b.key));
    diff.ttl_changed.sort_by(|a, b| a.key.cmp(&b.key));
    diff.added.sort();
    diff.removed.sort();

    Ok(diff)
}

/// Index entries by their canonical base64 `LedgerKey`, so a key expressed as
/// hex in one document and base64 in the other still compares equal.
fn index_entries(entries: &[SnapshotEntry]) -> BTreeMap<String, &SnapshotEntry> {
    let mut map = BTreeMap::new();
    for entry in entries {
        map.insert(canonical_key(&entry.key), entry);
    }
    map
}

/// Canonicalise a `LedgerKey` string (base64 or hex XDR) to standard base64.
///
/// Falls back to the trimmed input when it is not a decodable `LedgerKey`, so
/// hand-written or opaque keys still compare by exact string.
pub(crate) fn canonical_key(key: &str) -> String {
    let trimmed = key.trim();
    let Ok(decoded) = sdkt_xdr::decode_ledger_key(trimmed) else {
        return trimmed.to_string();
    };
    let mut buf = Vec::new();
    let mut limited = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    if decoded.write_xdr(&mut limited).is_err() {
        return trimmed.to_string();
    }
    base64::engine::general_purpose::STANDARD.encode(&buf)
}

// ---------------------------------------------------------------------------
// capture_snapshot
// ---------------------------------------------------------------------------

/// Capture a snapshot of a contract's storage.
///
/// `extra_keys` are additional ledger keys (base64/hex XDR) beyond the always
/// tracked contract instance singleton. TTL and rent come from
/// [`sdkt_rpc::get_ttl_info_for_keys`] and the value from
/// [`sdkt_rpc::read_contract_state`] — the same ledger-key capture path the rest
/// of the storage surface uses, so both see identical keys.
pub async fn capture_snapshot(
    client: &SorobanRpcClient,
    contract_id: &str,
    extra_keys: &[String],
) -> Result<StorageSnapshot, StorageError> {
    let ttl_info = sdkt_rpc::get_ttl_info_for_keys(client, contract_id, extra_keys).await?;
    let captured_at_ledger = client.get_ledger().await.ok().map(|info| info.sequence);

    let mut entries = Vec::with_capacity(ttl_info.entries.len());
    for entry in &ttl_info.entries {
        let class = classify_key(&entry.key);
        let value = match sdkt_rpc::read_contract_state(client, contract_id, &entry.key, None).await
        {
            Ok(state) => state
                .value
                .get("xdr")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            // Non-contract-data keys (accounts, code) carry no `ScVal` value to
            // capture. Every other read failure is a real error and must not be
            // silently recorded as "no value".
            Err(_) if class == StorageClass::Other => None,
            Err(e) => return Err(e.into()),
        };

        entries.push(SnapshotEntry {
            key: entry.key.clone(),
            label: None,
            class,
            durability: durability_label(class),
            current_ttl: entry.current_ttl,
            extension_cost_stroops: entry.extension_cost_stroops,
            value,
        });
    }

    Ok(StorageSnapshot {
        contract_id: contract_id.to_string(),
        captured_at_ledger,
        entries,
    })
}

fn durability_label(class: StorageClass) -> Option<String> {
    match class {
        StorageClass::Instance | StorageClass::Persistent => Some("persistent".to_string()),
        StorageClass::Temporary => Some("temporary".to_string()),
        StorageClass::Other => None,
    }
}

// ---------------------------------------------------------------------------
// ExtendPlan
// ---------------------------------------------------------------------------

/// A non-mutating remediation plan derived from a [`SnapshotDiff`].
///
/// The plan identifies which ledger keys need TTL extension and suggests a
/// `--ledgers` value the operator can pass directly to `sdkt storage extend`.
/// **Nothing is signed or submitted.**
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtendPlan {
    /// Contract whose storage needs remediation.
    pub contract_id: String,
    /// Ledger keys (base64 XDR) that should be included in the extend footprint.
    /// Empty when there is nothing to remediate.
    pub keys: Vec<String>,
    /// Suggested value for `--ledgers` (relative TTL extension).
    ///
    /// When there are actionable entries the suggestion is
    /// `max(DEFAULT_SUGGESTED_LEDGERS, min_remaining_ttl + DEFAULT_SUGGESTED_LEDGERS)`
    /// rounded to [`DEFAULT_SUGGESTED_LEDGERS`] when there are no remaining TTL
    /// signals (e.g. all entries are `Removed`).
    pub suggested_ledgers: u32,
    /// Human-readable reason explaining how `suggested_ledgers` was chosen.
    pub suggested_ledgers_reason: String,
}

// ---------------------------------------------------------------------------
// derive_extend_plan
// ---------------------------------------------------------------------------

/// Derive a non-mutating [`ExtendPlan`] from a [`SnapshotDiff`].
///
/// Only the `Removed` and `ExpiringSoon` entries contribute to the plan; no
/// transaction is built, signed, or submitted.
///
/// # Empty plan
/// When the diff has no actionable entries the returned plan has an empty
/// `keys` list, `suggested_ledgers` of [`DEFAULT_SUGGESTED_LEDGERS`], and a
/// clear reason string.  The caller should exit 0 in this case.
///
/// # Suggested ledger horizon
/// The heuristic is:
/// - Collect the `new_ttl` of all `ExpiringSoon` entries (removed entries have
///   no remaining TTL).
/// - Compute the minimum remaining TTL across those entries.
/// - Suggest `min_remaining_ttl + DEFAULT_SUGGESTED_LEDGERS` so the extension
///   carries entries comfortably past the threshold.
/// - When there are no `ExpiringSoon` entries (only `Removed`), default to
///   [`DEFAULT_SUGGESTED_LEDGERS`].
pub fn derive_extend_plan(diff: &SnapshotDiff) -> ExtendPlan {
    let actionable: Vec<&DiffEntry> = diff.actionable().collect();

    if actionable.is_empty() {
        return ExtendPlan {
            contract_id: diff.contract_id.clone(),
            keys: vec![],
            suggested_ledgers: DEFAULT_SUGGESTED_LEDGERS,
            suggested_ledgers_reason: "No actionable entries; nothing to extend.".into(),
        };
    }

    // Collect the keys for the plan.
    let keys: Vec<String> = actionable.iter().map(|e| e.key.clone()).collect();

    // Derive suggested ledgers from the minimum remaining TTL of expiring-soon
    // entries.  Removed entries have no remaining TTL and are excluded.
    let min_remaining: Option<u32> = actionable
        .iter()
        .filter_map(|e| {
            if e.status == DiffStatus::ExpiringSoon {
                e.new_ttl
            } else {
                None
            }
        })
        .min();

    let (suggested_ledgers, suggested_ledgers_reason) = match min_remaining {
        Some(min_ttl) => {
            let suggested = min_ttl.saturating_add(DEFAULT_SUGGESTED_LEDGERS);
            (
                suggested,
                format!(
                    "Minimum remaining TTL of expiring entries is {} ledgers; \
                     suggested {} (min_ttl + {} default horizon).",
                    min_ttl, suggested, DEFAULT_SUGGESTED_LEDGERS
                ),
            )
        }
        None => (
            DEFAULT_SUGGESTED_LEDGERS,
            format!(
                "All actionable entries are removed (no remaining TTL); \
                 defaulting to {} ledgers (~30 days).",
                DEFAULT_SUGGESTED_LEDGERS
            ),
        ),
    };

    ExtendPlan {
        contract_id: diff.contract_id.clone(),
        keys,
        suggested_ledgers,
        suggested_ledgers_reason,
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    const CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

    fn snap(entries: Vec<(&str, u32)>) -> StorageSnapshot {
        StorageSnapshot::from_entries(
            CONTRACT,
            entries
                .into_iter()
                .map(|(k, ttl)| SnapshotEntry {
                    key: k.to_string(),
                    current_ttl: ttl,
                    ..Default::default()
                })
                .collect(),
        )
    }

    // -----------------------------------------------------------------------
    // diff_snapshots
    // -----------------------------------------------------------------------

    #[test]
    fn diff_mismatched_contract_ids_returns_error() {
        let old = StorageSnapshot {
            contract_id: "CCONTRACTA".to_string(),
            ..Default::default()
        };
        let new = StorageSnapshot {
            contract_id: "CCONTRACTB".to_string(),
            ..Default::default()
        };
        let err = diff_snapshots(&old, &new).unwrap_err();
        assert!(matches!(
            err,
            StorageError::ContractIdMismatch { old, new }
                if old == "CCONTRACTA" && new == "CCONTRACTB"
        ));
    }

    #[test]
    fn diff_empty_old_and_new_is_empty() {
        let old = snap(vec![]);
        let new = snap(vec![]);
        let diff = diff_snapshots(&old, &new).unwrap();
        assert!(diff.entries.is_empty());
        assert_eq!(diff.changed(), 0);
        assert_eq!(diff.unchanged, 0);
    }

    #[test]
    fn diff_all_unchanged_when_ttl_above_threshold() {
        let ttl = EXPIRING_SOON_LEDGERS + 1;
        let old = snap(vec![("keyA", ttl), ("keyB", ttl)]);
        let new = snap(vec![("keyA", ttl), ("keyB", ttl)]);
        let diff = diff_snapshots(&old, &new).unwrap();
        assert_eq!(diff.entries.len(), 2);
        assert!(diff
            .entries
            .iter()
            .all(|e| e.status == DiffStatus::Unchanged));
        assert!(diff.actionable().count() == 0);
        assert_eq!(diff.unchanged, 2);
    }

    #[test]
    fn diff_removed_entry_is_flagged() {
        let old = snap(vec![("keyA", 50_000), ("keyB", 50_000)]);
        let new = snap(vec![("keyA", 49_000)]);
        let diff = diff_snapshots(&old, &new).unwrap();

        let removed: Vec<_> = diff
            .entries
            .iter()
            .filter(|e| e.status == DiffStatus::Removed)
            .collect();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].key, "keyB");
        assert_eq!(removed[0].old_ttl, Some(50_000));
        assert_eq!(removed[0].new_ttl, None);
        // Entering the TTL change set only means TTL moved on a *live* entry:
        // keyB is gone, so it is `removed`, not a TTL delta.
        assert_eq!(diff.removed, vec!["keyB".to_string()]);
        assert_eq!(diff.ttl_changed.len(), 1);
        assert_eq!(diff.ttl_changed[0].key, "keyA");
    }

    #[test]
    fn diff_expiring_soon_entry_is_flagged() {
        let old = snap(vec![("keyA", 50_000)]);
        let new = snap(vec![("keyA", EXPIRING_SOON_LEDGERS - 1)]);
        let diff = diff_snapshots(&old, &new).unwrap();

        assert_eq!(diff.entries.len(), 1);
        assert_eq!(diff.entries[0].status, DiffStatus::ExpiringSoon);
        assert_eq!(diff.entries[0].new_ttl, Some(EXPIRING_SOON_LEDGERS - 1));
    }

    #[test]
    fn diff_boundary_ttl_equals_threshold_is_expiring_soon() {
        // TTL == EXPIRING_SOON_LEDGERS is NOT below the threshold, so it is
        // Unchanged.  The test documents the boundary precisely.
        let old = snap(vec![("keyA", 50_000)]);
        let new = snap(vec![("keyA", EXPIRING_SOON_LEDGERS)]);
        let diff = diff_snapshots(&old, &new).unwrap();
        // Exactly at threshold → Unchanged (strict less-than in the check).
        assert_eq!(diff.entries[0].status, DiffStatus::Unchanged);
    }

    #[test]
    fn diff_n_entries_mixed_statuses() {
        let old = snap(vec![("k1", 100_000), ("k2", 100_000), ("k3", 100_000)]);
        let new = snap(vec![
            ("k1", 100_000), // unchanged
            ("k2", EXPIRING_SOON_LEDGERS - 100), // expiring soon
                             // k3 removed
        ]);
        let diff = diff_snapshots(&old, &new).unwrap();
        assert_eq!(diff.entries.len(), 3);

        let by_key: std::collections::HashMap<_, _> = diff
            .entries
            .iter()
            .map(|e| (e.key.as_str(), e.status))
            .collect();
        assert_eq!(by_key["k1"], DiffStatus::Unchanged);
        assert_eq!(by_key["k2"], DiffStatus::ExpiringSoon);
        assert_eq!(by_key["k3"], DiffStatus::Removed);
    }

    #[test]
    fn diff_output_is_deterministic() {
        let old = snap(vec![("zz", 50_000), ("aa", 50_000)]);
        let new = snap(vec![("zz", 50_000), ("aa", 50_000)]);
        let d1 = diff_snapshots(&old, &new).unwrap();
        let d2 = diff_snapshots(&old, &new).unwrap();
        assert_eq!(d1, d2);
        // Keys should be sorted.
        assert_eq!(d1.entries[0].key, "aa");
        assert_eq!(d1.entries[1].key, "zz");
    }

    #[test]
    fn diff_preserves_label_without_using_it_as_identity() {
        let mut old = snap(vec![("keyA", 50_000)]);
        let mut new = snap(vec![("keyA", 3_000)]);
        old.entries[0].label = Some("old label".into());
        new.entries[0].label = Some("DataKey::Balance(u32)".into());
        let labeled = diff_snapshots(&old, &new).unwrap();
        assert_eq!(labeled.entries.len(), 1);
        assert_eq!(labeled.entries[0].key, "keyA");
        assert_eq!(
            labeled.entries[0].label.as_deref(),
            Some("DataKey::Balance(u32)")
        );
        old.entries[0].label = None;
        new.entries[0].label = None;
        let plain = diff_snapshots(&old, &new).unwrap();
        assert_eq!(plain.entries[0].status, labeled.entries[0].status);
        assert_eq!(plain.entries[0].old_ttl, labeled.entries[0].old_ttl);
        assert_eq!(plain.entries[0].new_ttl, labeled.entries[0].new_ttl);
    }

    // -----------------------------------------------------------------------
    // Value-aware diffing
    // -----------------------------------------------------------------------

    fn entry(key: &str, value: Option<&str>, ttl: u32, rent: u64) -> SnapshotEntry {
        SnapshotEntry {
            key: key.to_string(),
            class: StorageClass::Persistent,
            durability: Some("persistent".to_string()),
            current_ttl: ttl,
            extension_cost_stroops: rent,
            value: value.map(str::to_string),
            ..Default::default()
        }
    }

    fn snapshot(entries: Vec<SnapshotEntry>) -> StorageSnapshot {
        StorageSnapshot {
            contract_id: CONTRACT.to_string(),
            captured_at_ledger: Some(1000),
            entries,
        }
    }

    #[test]
    fn value_only_change_is_a_value_delta_not_ttl() {
        let base = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("BBBB"), 20000, 2_000_000)]);

        let diff = diff_snapshots(&base, &live).unwrap();

        assert_eq!(diff.changed(), 1);
        assert_eq!(diff.unchanged, 0);
        assert!(diff.ttl_changed.is_empty(), "TTL matched, so no TTL delta");
        assert_eq!(diff.value_changed.len(), 1);
        let delta = &diff.value_changed[0];
        assert_eq!(delta.key, "k1");
        assert_eq!(delta.before.as_deref(), Some("AAAA"));
        assert_eq!(delta.after.as_deref(), Some("BBBB"));
        assert_eq!(diff.entries[0].status, DiffStatus::ValueChanged);
    }

    #[test]
    fn matching_value_and_ttl_is_unchanged() {
        let base = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);

        let diff = diff_snapshots(&base, &live).unwrap();

        assert_eq!(diff.changed(), 0);
        assert_eq!(diff.unchanged, 1);
        assert_eq!(diff.entries[0].status, DiffStatus::Unchanged);
    }

    #[test]
    fn ttl_only_change_reports_only_ttl_delta() {
        let base = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("AAAA"), 15000, 1_500_000)]);

        let diff = diff_snapshots(&base, &live).unwrap();

        assert_eq!(diff.changed(), 1);
        assert!(
            diff.value_changed.is_empty(),
            "value matched, so no value delta"
        );
        assert_eq!(diff.ttl_changed.len(), 1);
        let delta = &diff.ttl_changed[0];
        assert_eq!(delta.before_ttl, 20000);
        assert_eq!(delta.after_ttl, 15000);
        assert_eq!(delta.before_extension_cost_stroops, 2_000_000);
        assert_eq!(delta.after_extension_cost_stroops, 1_500_000);
        // 15 000 ledgers is below the expiring-soon threshold.
        assert_eq!(diff.entries[0].status, DiffStatus::ExpiringSoon);
    }

    #[test]
    fn rent_change_without_ttl_change_is_a_ttl_delta() {
        let base = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_500_000)]);

        let diff = diff_snapshots(&base, &live).unwrap();

        assert!(diff.value_changed.is_empty());
        assert_eq!(diff.ttl_changed.len(), 1);
        assert_eq!(diff.unchanged, 0);
    }

    #[test]
    fn value_less_snapshot_keeps_ttl_only_semantics() {
        // A document written before value capture existed has no `value` field.
        let base = snapshot(vec![entry("k1", None, 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("BBBB"), 20000, 2_000_000)]);

        let diff = diff_snapshots(&base, &live).unwrap();

        assert_eq!(
            diff.changed(),
            0,
            "with no captured base value there is nothing to compare"
        );
        assert_eq!(diff.unchanged, 1);
    }

    #[test]
    fn added_and_removed_keys_are_reported() {
        let base = snapshot(vec![
            entry("gone", Some("AAAA"), 10, 1000),
            entry("same", None, 5, 500),
        ]);
        let live = snapshot(vec![
            entry("same", None, 5, 500),
            entry("fresh", None, 7, 700),
        ]);

        let diff = diff_snapshots(&base, &live).unwrap();

        assert_eq!(diff.removed, vec!["gone".to_string()]);
        assert_eq!(diff.added, vec!["fresh".to_string()]);
        assert_eq!(diff.unchanged, 1);
        assert_eq!(diff.changed(), 2);
    }

    #[test]
    fn keys_are_compared_by_canonical_form() {
        // The same ledger key, written with surrounding whitespace, must match.
        let key = sdkt_xdr::encode_ledger_key(&sdkt_xdr::LedgerKeyParams::ContractDataEntry {
            contract: CONTRACT.to_string(),
            key: stellar_xdr::ScVal::U32(1),
            durability: stellar_xdr::ContractDataDurability::Persistent,
        })
        .unwrap();

        let base = snapshot(vec![entry(&key, Some("AAAA"), 10, 1000)]);
        let live = snapshot(vec![entry(&format!("  {key}  "), Some("BBBB"), 10, 1000)]);

        let diff = diff_snapshots(&base, &live).unwrap();

        assert_eq!(diff.value_changed.len(), 1);
        assert_eq!(diff.removed.len(), 0);
        assert_eq!(diff.added.len(), 0);
    }

    #[test]
    fn value_less_document_deserializes_with_value_none() {
        // Snapshot JSON written before this change: no `value` key present.
        let json = format!(
            r#"{{"contract_id":"{CONTRACT}","entries":[
                {{"key":"some-key","class":"persistent","durability":"persistent",
                  "current_ttl":20000,"extension_cost_stroops":2000000}}
            ]}}"#
        );

        let snapshot: StorageSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].value, None);
        assert_eq!(snapshot.entries[0].current_ttl, 20000);

        // A minimal document with only a key also parses (defaulted fields).
        let minimal: StorageSnapshot = serde_json::from_str(&format!(
            r#"{{"contract_id":"{CONTRACT}","entries":[{{"key":"k"}}]}}"#
        ))
        .unwrap();
        assert_eq!(minimal.entries[0].value, None);
        assert_eq!(minimal.entries[0].current_ttl, 0);
    }

    #[test]
    fn captured_value_is_serialized_only_when_present() {
        let with_value = snapshot(vec![entry("k1", Some("AAAA"), 10, 100)]);
        let json = serde_json::to_string(&with_value).unwrap();
        assert!(json.contains(r#""value":"AAAA""#));

        let without_value = snapshot(vec![entry("k1", None, 10, 100)]);
        let json = serde_json::to_string(&without_value).unwrap();
        assert!(!json.contains("\"value\""));
    }

    #[test]
    fn empty_change_sets_are_omitted_from_json() {
        let base = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let diff = diff_snapshots(&base, &live).unwrap();
        let json = serde_json::to_string(&diff).unwrap();
        assert!(!json.contains("\"value_changed\""));
        assert!(!json.contains("\"ttl_changed\""));
        assert!(!json.contains("\"added\""));
        assert!(!json.contains("\"removed\""));
        assert!(json.contains("\"unchanged\":1"));
    }

    /// Encode an `ScVal` as the base64 XDR string `read_contract_state` returns.
    fn scval_base64(val: &stellar_xdr::ScVal) -> String {
        let mut buf = Vec::new();
        let mut limited = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
        val.write_xdr(&mut limited).unwrap();
        base64::engine::general_purpose::STANDARD.encode(&buf)
    }

    /// Encode a `ContractData` `LedgerEntry` whose captured value is `val`.
    fn contract_data_entry_xdr(val: stellar_xdr::ScVal) -> String {
        use stellar_xdr::{
            ContractDataDurability, ContractDataEntry, ContractId, ExtensionPoint, Hash,
            LedgerEntry, LedgerEntryData, LedgerEntryExt, ScAddress,
        };

        let ledger_entry = LedgerEntry {
            last_modified_ledger_seq: 100,
            data: LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: stellar_xdr::ScVal::U32(1),
                durability: ContractDataDurability::Persistent,
                val,
            }),
            ext: LedgerEntryExt::V0,
        };
        let mut buf = Vec::new();
        let mut limited = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
        ledger_entry.write_xdr(&mut limited).unwrap();
        base64::engine::general_purpose::STANDARD.encode(&buf)
    }

    fn contract_data_key(key: stellar_xdr::ScVal) -> String {
        sdkt_xdr::encode_ledger_key(&sdkt_xdr::LedgerKeyParams::ContractDataEntry {
            contract: CONTRACT.to_string(),
            key,
            durability: stellar_xdr::ContractDataDurability::Persistent,
        })
        .unwrap()
    }

    /// Read one HTTP request in full (headers + body per Content-Length).
    fn read_request(sock: &mut std::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = sock.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&data).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text[..end]
                    .lines()
                    .filter_map(|l| l.split_once(':'))
                    .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if data.len() >= end + 4 + len {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&data).to_string()
    }

    /// Mock RPC that echoes one entry per requested key, taking each entry's
    /// captured value from `values` (default 100) and a fixed TTL.
    fn mock_snapshot_rpc(
        values: Arc<Mutex<BTreeMap<String, u32>>>,
    ) -> (String, Arc<Mutex<Vec<Vec<String>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let queried: Arc<Mutex<Vec<Vec<String>>>> = Default::default();
        let queried_thread = queried.clone();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut sock) = conn else { break };
                let req = read_request(&mut sock);
                let body = req.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
                let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
                let method = parsed
                    .as_ref()
                    .and_then(|v| v["method"].as_str())
                    .unwrap_or_default()
                    .to_string();

                let resp_body = match method.as_str() {
                    "getLatestLedger" => {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":22,"sequence":1200}}"#
                            .to_string()
                    }
                    "getLedgerEntries" => {
                        let keys: Vec<String> = parsed
                            .as_ref()
                            .and_then(|v| v["params"]["keys"].as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|k| k.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        queried_thread.lock().unwrap().push(keys.clone());

                        let values = values.lock().unwrap();
                        let entries: Vec<serde_json::Value> = keys
                            .iter()
                            .map(|k| {
                                let val = *values.get(k).unwrap_or(&100);
                                serde_json::json!({
                                    "key": k,
                                    "xdr": contract_data_entry_xdr(stellar_xdr::ScVal::U32(val)),
                                    "lastModifiedLedgerSeq": 1000,
                                    "liveUntilLedgerSeq": 21200,
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "result": { "entries": entries, "latestLedger": 1200 }
                        })
                        .to_string()
                    }
                    _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not found"}}"#
                        .to_string(),
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        (url, queried)
    }

    #[tokio::test]
    async fn capture_snapshot_records_each_entry_value() {
        let persistent_key = contract_data_key(stellar_xdr::ScVal::U32(10));
        let instance_key = contract_data_key(stellar_xdr::ScVal::LedgerKeyContractInstance);

        let mut initial = BTreeMap::new();
        initial.insert(persistent_key.clone(), 4242);
        initial.insert(instance_key.clone(), 7);
        let values = Arc::new(Mutex::new(initial));

        let (url, _queried) = mock_snapshot_rpc(values.clone());
        let client = SorobanRpcClient::new(&url);

        let snapshot = capture_snapshot(&client, CONTRACT, std::slice::from_ref(&persistent_key))
            .await
            .unwrap();

        assert_eq!(snapshot.contract_id, CONTRACT);
        assert_eq!(snapshot.captured_at_ledger, Some(1200));
        assert_eq!(snapshot.entries.len(), 2);

        let persistent = snapshot
            .entries
            .iter()
            .find(|e| e.key == persistent_key)
            .expect("persistent entry captured");
        assert_eq!(persistent.class, StorageClass::Persistent);
        assert_eq!(persistent.durability.as_deref(), Some("persistent"));
        assert_eq!(persistent.current_ttl, 20000);
        assert_eq!(
            persistent.value.as_deref(),
            Some(scval_base64(&stellar_xdr::ScVal::U32(4242)).as_str())
        );

        let instance = snapshot
            .entries
            .iter()
            .find(|e| e.key == instance_key)
            .expect("instance entry captured");
        assert_eq!(instance.class, StorageClass::Instance);
    }

    #[tokio::test]
    async fn capture_then_diff_reports_value_only_change() {
        let persistent_key = contract_data_key(stellar_xdr::ScVal::U32(10));
        let instance_key = contract_data_key(stellar_xdr::ScVal::LedgerKeyContractInstance);

        let mut initial = BTreeMap::new();
        initial.insert(persistent_key.clone(), 100);
        initial.insert(instance_key.clone(), 7);
        let values = Arc::new(Mutex::new(initial));

        let (url, _queried) = mock_snapshot_rpc(values.clone());
        let client = SorobanRpcClient::new(&url);

        let base = capture_snapshot(&client, CONTRACT, std::slice::from_ref(&persistent_key))
            .await
            .unwrap();

        // Only the persistent entry's value changes; its TTL and the whole
        // instance entry stay identical.
        values.lock().unwrap().insert(persistent_key.clone(), 0);

        let live = capture_snapshot(&client, CONTRACT, std::slice::from_ref(&persistent_key))
            .await
            .unwrap();

        let diff = diff_snapshots(&base, &live).unwrap();

        assert_eq!(diff.changed(), 1, "{diff:?}");
        assert_eq!(diff.unchanged, 1);
        assert!(diff.ttl_changed.is_empty());
        assert_eq!(diff.value_changed.len(), 1);
        assert_eq!(diff.value_changed[0].key, persistent_key);
        assert_eq!(
            diff.value_changed[0].before.as_deref(),
            Some(scval_base64(&stellar_xdr::ScVal::U32(100)).as_str())
        );
        assert_eq!(
            diff.value_changed[0].after.as_deref(),
            Some(scval_base64(&stellar_xdr::ScVal::U32(0)).as_str())
        );
    }

    // -----------------------------------------------------------------------
    // derive_extend_plan
    // -----------------------------------------------------------------------

    #[test]
    fn plan_empty_diff_produces_empty_plan() {
        let diff = SnapshotDiff {
            contract_id: CONTRACT.to_string(),
            ..Default::default()
        };
        let plan = derive_extend_plan(&diff);
        assert!(plan.keys.is_empty());
        assert_eq!(plan.suggested_ledgers, DEFAULT_SUGGESTED_LEDGERS);
        assert!(plan.suggested_ledgers_reason.contains("nothing to extend"));
    }

    #[test]
    fn plan_one_removed_entry() {
        let diff = SnapshotDiff {
            contract_id: CONTRACT.to_string(),
            entries: vec![DiffEntry {
                key: "k1".to_string(),
                status: DiffStatus::Removed,
                old_ttl: Some(50_000),
                new_ttl: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        let plan = derive_extend_plan(&diff);
        assert_eq!(plan.keys, vec!["k1"]);
        // No remaining TTL → default horizon.
        assert_eq!(plan.suggested_ledgers, DEFAULT_SUGGESTED_LEDGERS);
        assert!(plan.suggested_ledgers_reason.contains("removed"));
    }

    #[test]
    fn plan_one_expiring_entry_scales_with_remaining_ttl() {
        let remaining = 5_000u32;
        let diff = SnapshotDiff {
            contract_id: CONTRACT.to_string(),
            entries: vec![DiffEntry {
                key: "k1".to_string(),
                status: DiffStatus::ExpiringSoon,
                old_ttl: Some(50_000),
                new_ttl: Some(remaining),
                ..Default::default()
            }],
            ..Default::default()
        };
        let plan = derive_extend_plan(&diff);
        assert_eq!(plan.keys, vec!["k1"]);
        assert_eq!(
            plan.suggested_ledgers,
            remaining + DEFAULT_SUGGESTED_LEDGERS
        );
        assert!(plan
            .suggested_ledgers_reason
            .contains(&remaining.to_string()));
    }

    #[test]
    fn plan_n_entries_uses_minimum_remaining_ttl() {
        let diff = SnapshotDiff {
            contract_id: CONTRACT.to_string(),
            entries: vec![
                DiffEntry {
                    key: "k1".to_string(),
                    status: DiffStatus::ExpiringSoon,
                    old_ttl: Some(50_000),
                    new_ttl: Some(10_000),
                    ..Default::default()
                },
                DiffEntry {
                    key: "k2".to_string(),
                    status: DiffStatus::ExpiringSoon,
                    old_ttl: Some(50_000),
                    new_ttl: Some(1_000), // minimum
                    ..Default::default()
                },
                DiffEntry {
                    key: "k3".to_string(),
                    status: DiffStatus::Removed,
                    old_ttl: Some(50_000),
                    new_ttl: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let plan = derive_extend_plan(&diff);
        assert_eq!(plan.keys.len(), 3);
        // min remaining = 1000; suggestion = 1000 + DEFAULT_SUGGESTED_LEDGERS
        assert_eq!(plan.suggested_ledgers, 1_000 + DEFAULT_SUGGESTED_LEDGERS);
    }

    #[test]
    fn plan_unchanged_entries_not_included() {
        let diff = SnapshotDiff {
            contract_id: CONTRACT.to_string(),
            entries: vec![
                DiffEntry {
                    key: "unchanged".to_string(),
                    status: DiffStatus::Unchanged,
                    old_ttl: Some(100_000),
                    new_ttl: Some(99_000),
                    ..Default::default()
                },
                DiffEntry {
                    key: "expiring".to_string(),
                    status: DiffStatus::ExpiringSoon,
                    old_ttl: Some(20_000),
                    new_ttl: Some(5_000),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let plan = derive_extend_plan(&diff);
        assert_eq!(plan.keys, vec!["expiring"]);
    }

    #[test]
    fn plan_json_has_stable_field_names() {
        let plan = ExtendPlan {
            contract_id: CONTRACT.to_string(),
            keys: vec!["k1".to_string(), "k2".to_string()],
            suggested_ledgers: 518_400,
            suggested_ledgers_reason: "test".to_string(),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["contract_id"], CONTRACT);
        assert_eq!(v["suggested_ledgers"], 518_400);
        assert!(v["keys"].is_array());
        assert_eq!(v["keys"].as_array().unwrap().len(), 2);
        assert!(v["suggested_ledgers_reason"].is_string());
    }

    #[test]
    fn snapshot_from_report_preserves_all_entries() {
        use crate::types::{StorageClass, StorageEntry, StorageReport};
        let report = StorageReport {
            contract_id: CONTRACT.to_string(),
            total_entries: 2,
            instance_entries: 1,
            persistent_entries: 1,
            temporary_entries: 0,
            other_entries: 0,
            total_size_bytes: None,
            ttl_summary: None,
            entries: vec![
                StorageEntry {
                    key: "key1".to_string(),
                    label: None,
                    class: StorageClass::Instance,
                    current_ttl: 10_000,
                    days_remaining: 0,
                    extension_cost_stroops: 0,
                },
                StorageEntry {
                    key: "key2".to_string(),
                    label: None,
                    class: StorageClass::Persistent,
                    current_ttl: 5_000,
                    days_remaining: 0,
                    extension_cost_stroops: 1234,
                },
            ],
        };
        let snap = StorageSnapshot::from_report(&report).unwrap();
        assert_eq!(snap.contract_id, CONTRACT);
        assert_eq!(snap.entries.len(), 2);
        assert_eq!(snap.entries[0].key, "key1");
        assert_eq!(snap.entries[0].current_ttl, 10_000);
        assert_eq!(snap.entries[0].class, StorageClass::Instance);
        assert_eq!(snap.entries[0].durability.as_deref(), Some("persistent"));
        // Rent and value are carried over from the report / left absent.
        assert_eq!(snap.entries[1].extension_cost_stroops, 1234);
        assert_eq!(snap.entries[1].value, None);
    }

    #[test]
    fn snapshot_from_report_rejects_empty_entries_when_total_entries_positive() {
        use crate::types::StorageReport;
        let report = StorageReport {
            contract_id: CONTRACT.to_string(),
            total_entries: 3,
            ..Default::default()
        };
        let err = StorageSnapshot::from_report(&report).unwrap_err();
        assert!(matches!(err, StorageError::Parse(msg) if msg.contains("total_entries (3) > 0")));
    }
}
