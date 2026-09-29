//! Snapshot-diffing and TTL extension-plan derivation for contract storage.
//!
//! # Overview
//!
//! A [`StorageSnapshot`] is a point-in-time capture of a contract's storage
//! entries (key + TTL), obtained from [`crate::StorageReport`] or built by the
//! CLI directly from an RPC response.
//!
//! [`diff_snapshots`] compares an **old** (baseline) snapshot against a **new**
//! (current) one and classifies each entry as:
//! - [`DiffStatus::Removed`]  — entry present in old, absent in new (expired or
//!   otherwise gone).
//! - [`DiffStatus::ExpiringSoon`] — entry still live in new but with a TTL below
//!   [`EXPIRING_SOON_LEDGERS`] (≈ 1 day).
//! - [`DiffStatus::Unchanged`] — entry live and above the threshold.
//!
//! [`derive_extend_plan`] turns the diff result into a non-mutating
//! [`ExtendPlan`]: the set of ledger keys that need remediation, the contract
//! they belong to, and a suggested `--ledgers` value the operator can pass
//! directly to `sdkt storage extend`.
//!
//! # Guarantees
//! - No RPC mutation methods are called in this module.
//! - An empty diff (no removed or expiring entries) produces an [`ExtendPlan`]
//!   with an empty key list and exits cleanly.

use crate::error::StorageError;
use crate::types::StorageReport;
use serde::{Deserialize, Serialize};

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
/// The snapshot intentionally stores only the data needed for diffing
/// (`key` + `current_ttl`).  Callers may build it from a [`StorageReport`]
/// via [`StorageSnapshot::from_report`], or construct it directly in tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StorageSnapshot {
    /// The on-chain contract identifier (C… StrKey or hex).
    pub contract_id: String,
    /// Entries captured at the time this snapshot was taken.
    pub entries: Vec<SnapshotEntry>,
}

/// A single entry inside a [`StorageSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotEntry {
    /// Base64 XDR encoded `LedgerKey`.
    pub key: String,
    /// Readable ABI-derived key label, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// TTL in ledgers relative to the ledger at snapshot time.
    pub current_ttl: u32,
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
            entries: report
                .entries
                .iter()
                .map(|e| SnapshotEntry {
                    key: e.key.clone(),
                    label: e.label.clone(),
                    current_ttl: e.current_ttl,
                })
                .collect(),
        })
    }

    /// Build a snapshot from raw `(key, ttl)` pairs (useful in tests and CLIs
    /// that already have the data from RPC without going through the analyzer).
    pub fn from_entries(contract_id: impl Into<String>, entries: Vec<SnapshotEntry>) -> Self {
        Self {
            contract_id: contract_id.into(),
            entries,
        }
    }
}

// ---------------------------------------------------------------------------
// Diff types
// ---------------------------------------------------------------------------

/// Classification of a single entry in a snapshot diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffStatus {
    /// Entry was present in the old snapshot but is absent in the new one.
    /// This means it expired or was otherwise removed from the ledger.
    Removed,
    /// Entry is still live in the new snapshot but its TTL is below
    /// [`EXPIRING_SOON_LEDGERS`].  Action is recommended before it expires.
    ExpiringSoon,
    /// Entry is still live and its TTL is above the expiring-soon threshold.
    Unchanged,
}

/// A single entry in a [`SnapshotDiff`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SnapshotDiff {
    /// Contract these snapshots describe.
    pub contract_id: String,
    /// All entries across both snapshots.
    pub entries: Vec<DiffEntry>,
}

impl SnapshotDiff {
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
/// Entries are keyed by their base64 XDR `LedgerKey`.  The diff is computed as
/// follows:
///
/// 1. Every key in the old snapshot that is absent in the new snapshot →
///    [`DiffStatus::Removed`].
/// 2. Every key in the new snapshot whose TTL < [`EXPIRING_SOON_LEDGERS`] →
///    [`DiffStatus::ExpiringSoon`].
/// 3. All other keys present in the new snapshot → [`DiffStatus::Unchanged`].
///
/// Keys that appear only in the new snapshot (added entries) are reported as
/// [`DiffStatus::Unchanged`] (or `ExpiringSoon` if their TTL is low).  There is
/// intentionally no `Added` status because the diff is used exclusively to drive
/// remediation of *at-risk* entries.
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

    use std::collections::HashMap;

    let old_map: HashMap<&str, &SnapshotEntry> =
        old.entries.iter().map(|e| (e.key.as_str(), e)).collect();

    let new_map: HashMap<&str, &SnapshotEntry> =
        new.entries.iter().map(|e| (e.key.as_str(), e)).collect();

    let mut entries: Vec<DiffEntry> = Vec::new();

    // Entries present in old — may be removed or still alive.
    for (key, &old_entry) in &old_map {
        let old_ttl = old_entry.current_ttl;
        if let Some(&new_entry) = new_map.get(key) {
            let new_ttl = new_entry.current_ttl;
            let status = if new_ttl < EXPIRING_SOON_LEDGERS {
                DiffStatus::ExpiringSoon
            } else {
                DiffStatus::Unchanged
            };
            entries.push(DiffEntry {
                key: key.to_string(),
                label: new_entry.label.clone().or_else(|| old_entry.label.clone()),
                status,
                old_ttl: Some(old_ttl),
                new_ttl: Some(new_ttl),
            });
        } else {
            entries.push(DiffEntry {
                key: key.to_string(),
                label: old_entry.label.clone(),
                status: DiffStatus::Removed,
                old_ttl: Some(old_ttl),
                new_ttl: None,
            });
        }
    }

    // Entries that are new (only present in new snapshot).
    for (key, &new_entry) in &new_map {
        if !old_map.contains_key(key) {
            let new_ttl = new_entry.current_ttl;
            let status = if new_ttl < EXPIRING_SOON_LEDGERS {
                DiffStatus::ExpiringSoon
            } else {
                DiffStatus::Unchanged
            };
            entries.push(DiffEntry {
                key: key.to_string(),
                label: new_entry.label.clone(),
                status,
                old_ttl: None,
                new_ttl: Some(new_ttl),
            });
        }
    }

    // Stable sort so output is deterministic (by key, then by status).
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    Ok(SnapshotDiff {
        contract_id: old.contract_id.clone(),
        entries,
    })
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

    const CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

    fn snap(entries: Vec<(&str, u32)>) -> StorageSnapshot {
        StorageSnapshot::from_entries(
            CONTRACT,
            entries
                .into_iter()
                .map(|(k, ttl)| SnapshotEntry {
                    key: k.to_string(),
                    label: None,
                    current_ttl: ttl,
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
            entries: vec![],
        };
        let new = StorageSnapshot {
            contract_id: "CCONTRACTB".to_string(),
            entries: vec![],
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
    // derive_extend_plan
    // -----------------------------------------------------------------------

    #[test]
    fn plan_empty_diff_produces_empty_plan() {
        let diff = SnapshotDiff {
            contract_id: CONTRACT.to_string(),
            entries: vec![],
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
                label: None,
                status: DiffStatus::Removed,
                old_ttl: Some(50_000),
                new_ttl: None,
            }],
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
                label: None,
                status: DiffStatus::ExpiringSoon,
                old_ttl: Some(50_000),
                new_ttl: Some(remaining),
            }],
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
                    label: None,
                    status: DiffStatus::ExpiringSoon,
                    old_ttl: Some(50_000),
                    new_ttl: Some(10_000),
                },
                DiffEntry {
                    key: "k2".to_string(),
                    label: None,
                    status: DiffStatus::ExpiringSoon,
                    old_ttl: Some(50_000),
                    new_ttl: Some(1_000), // minimum
                },
                DiffEntry {
                    key: "k3".to_string(),
                    label: None,
                    status: DiffStatus::Removed,
                    old_ttl: Some(50_000),
                    new_ttl: None,
                },
            ],
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
                    label: None,
                    status: DiffStatus::Unchanged,
                    old_ttl: Some(100_000),
                    new_ttl: Some(99_000),
                },
                DiffEntry {
                    key: "expiring".to_string(),
                    label: None,
                    status: DiffStatus::ExpiringSoon,
                    old_ttl: Some(20_000),
                    new_ttl: Some(5_000),
                },
            ],
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
                    extension_cost_stroops: 0,
                },
            ],
        };
        let snap = StorageSnapshot::from_report(&report).unwrap();
        assert_eq!(snap.contract_id, CONTRACT);
        assert_eq!(snap.entries.len(), 2);
        assert_eq!(snap.entries[0].key, "key1");
        assert_eq!(snap.entries[0].current_ttl, 10_000);
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
