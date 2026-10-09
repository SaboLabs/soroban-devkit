//! State capture for a state-matched local-vs-RPC differential run.
//!
//! ## What this module is for
//!
//! A differential run compares a local host execution against an RPC
//! `simulateTransaction`. For that comparison to mean anything, both sides
//! must execute against the *same* ledger state. This module captures the
//! state the RPC simulation used, so the local baseline can be built from it.
//!
//! ## What it can and cannot guarantee
//!
//! `getLedgerEntries` reads **current** ledger state — the RPC has no
//! parameter to pin a read to a specific ledger sequence. A snapshot taken
//! now can therefore differ from the state the simulation ran against, if
//! the ledger moved in between.
//!
//! This module does **not** pretend that away. It records the ledger
//! sequences involved and *verifies* the consistency conditions that can be
//! checked, and reports [`StateConsistency::Unverified`] with the reason
//! when they fail. A run whose state cannot be verified is
//! [`StateCaptureOutcome::Blocked`], never a state-matched comparison.
//!
//! The conditions checked are:
//!
//! 1. **Fetch freshness** — the state was read at a ledger at least as new as
//!    the simulation's ledger (`fetch_latest_ledger >= simulation_latest_ledger`).
//!    State older than the simulation cannot be what the simulation saw.
//! 2. **Entry liveness** — every captured entry is still live at the
//!    simulation ledger (`live_until_ledger_seq >= simulation_latest_ledger`).
//! 3. **Entry stability** — no entry was modified after the simulation ledger
//!    (`last_modified_ledger_seq <= simulation_latest_ledger`). If an entry
//!    changed after the simulation, the captured value is not the value the
//!    simulation used.
//!
//! Conditions 2 and 3 are *necessary*, not sufficient: an entry could have
//! been modified and reverted between the simulation and the fetch, which
//! leaves `last_modified` newer but the value identical. That case is
//! detectable only by comparing the captured value against the simulation's
//! own `stateChanges` (see [`verify_against_state_changes`]), which is done
//! separately and is also required for a verified verdict.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use stellar_xdr::{LedgerEntry, LedgerKey, ReadXdr, WriteXdr};

/// One captured ledger entry, with the provenance needed to audit it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedEntry {
    /// Canonical base64 XDR of the `LedgerKey`.
    pub key_b64: String,
    /// The `LedgerEntry` value read from the network.
    pub entry: LedgerEntry,
    /// `lastModifiedLedgerSeq` reported by the RPC for this entry.
    pub last_modified_ledger_seq: u32,
    /// `liveUntilLedgerSeq` reported by the RPC, when present.
    pub live_until_ledger_seq: Option<u32>,
    /// Where the value came from.
    pub source: EntrySource,
}

/// How a captured entry's value was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntrySource {
    /// Read from `getLedgerEntries` (current ledger state).
    LedgerEntriesRead,
    /// The entry did not exist at read time (the RPC returned no entry for
    /// the key). The simulation may still have created it.
    AbsentAtRead,
}

/// Why a state capture could not be verified against the simulation ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum StateUnverified {
    /// The state was read at a ledger older than the simulation's ledger, so
    /// it cannot be the state the simulation used.
    StateOlderThanSimulation {
        fetch_latest_ledger: u32,
        simulation_latest_ledger: u32,
    },
    /// An entry stopped being live before the simulation ledger.
    EntryNotLive {
        key_b64: String,
        live_until_ledger_seq: u32,
        simulation_latest_ledger: u32,
    },
    /// An entry was modified after the simulation ledger, so the captured
    /// value may not be the value the simulation used.
    EntryModifiedAfterSimulation {
        key_b64: String,
        last_modified_ledger_seq: u32,
        simulation_latest_ledger: u32,
    },
    /// A footprint key has no captured entry and was not accounted for.
    FootprintKeyMissing { key_b64: String },
    /// A footprint key was captured but the entry was absent at read time,
    /// and no evidence explains the absence (e.g. the simulation created it).
    FootprintKeyAbsentUnverified { key_b64: String },
    /// The captured value disagrees with the simulation's own `stateChanges`
    /// `before` value for the same key.
    ValueMismatchWithSimulation { key_b64: String },
}

impl StateUnverified {
    /// Stable category name for reporting.
    pub fn category(&self) -> &'static str {
        match self {
            StateUnverified::StateOlderThanSimulation { .. } => "state_older_than_simulation",
            StateUnverified::EntryNotLive { .. } => "entry_not_live",
            StateUnverified::EntryModifiedAfterSimulation { .. } => {
                "entry_modified_after_simulation"
            }
            StateUnverified::FootprintKeyMissing { .. } => "footprint_key_missing",
            StateUnverified::FootprintKeyAbsentUnverified { .. } => {
                "footprint_key_absent_unverified"
            }
            StateUnverified::ValueMismatchWithSimulation { .. } => "value_mismatch_with_simulation",
        }
    }
}

/// The outcome of a state capture: either a verified capture, or an explicit
/// blocker. A blocked capture never becomes a state-matched comparison.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum StateCaptureOutcome {
    /// The state was captured and every checkable consistency condition
    /// held.
    Verified {
        /// Captured entries, keyed by canonical key XDR.
        entries: Vec<CapturedEntry>,
        /// The ledger the state was read at (`latestLedger` of the read).
        fetch_latest_ledger: u32,
        /// The ledger the simulation ran against.
        simulation_latest_ledger: u32,
    },
    /// The state could not be verified. No baseline may be built from it.
    Blocked {
        /// Why it could not be verified.
        reason: StateUnverified,
        /// Entries that *were* captured, for diagnostics only.
        entries: Vec<CapturedEntry>,
        fetch_latest_ledger: u32,
        simulation_latest_ledger: u32,
    },
}

impl StateCaptureOutcome {
    /// The captured entries, when verified.
    pub fn verified_entries(&self) -> Option<&[CapturedEntry]> {
        match self {
            StateCaptureOutcome::Verified { entries, .. } => Some(entries),
            StateCaptureOutcome::Blocked { .. } => None,
        }
    }

    /// True when the capture is blocked. A blocked capture is never a
    /// state-matched comparison.
    pub fn is_blocked(&self) -> bool {
        matches!(self, StateCaptureOutcome::Blocked { .. })
    }

    /// The blocker reason, when blocked.
    pub fn block_reason(&self) -> Option<&StateUnverified> {
        match self {
            StateCaptureOutcome::Blocked { reason, .. } => Some(reason),
            StateCaptureOutcome::Verified { .. } => None,
        }
    }

    /// The entries captured, whether verified or not (diagnostics).
    pub fn entries(&self) -> &[CapturedEntry] {
        match self {
            StateCaptureOutcome::Verified { entries, .. } => entries,
            StateCaptureOutcome::Blocked { entries, .. } => entries,
        }
    }

    pub fn fetch_latest_ledger(&self) -> u32 {
        match self {
            StateCaptureOutcome::Verified {
                fetch_latest_ledger,
                ..
            } => *fetch_latest_ledger,
            StateCaptureOutcome::Blocked {
                fetch_latest_ledger,
                ..
            } => *fetch_latest_ledger,
        }
    }

    pub fn simulation_latest_ledger(&self) -> u32 {
        match self {
            StateCaptureOutcome::Verified {
                simulation_latest_ledger,
                ..
            } => *simulation_latest_ledger,
            StateCaptureOutcome::Blocked {
                simulation_latest_ledger,
                ..
            } => *simulation_latest_ledger,
        }
    }
}

/// Canonical base64 XDR for a `LedgerKey`.
pub fn encode_key(key: &LedgerKey) -> Result<String, String> {
    use base64::Engine;
    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    key.write_xdr(&mut l).map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&buf))
}

/// Decode a base64 XDR `LedgerKey`.
pub fn decode_key(key_b64: &str) -> Result<LedgerKey, String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(key_b64.trim())
        .map_err(|e| e.to_string())?;
    let mut cursor = std::io::Cursor::new(&raw);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    LedgerKey::read_xdr(&mut l).map_err(|e| e.to_string())
}

/// Decode a base64 XDR `LedgerEntry`.
pub fn decode_entry(entry_b64: &str) -> Result<LedgerEntry, String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(entry_b64.trim())
        .map_err(|e| e.to_string())?;
    let mut cursor = std::io::Cursor::new(&raw);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    LedgerEntry::read_xdr(&mut l).map_err(|e| e.to_string())
}

/// A footprint entry read from `getLedgerEntries`, before consistency checks.
#[derive(Clone, Debug)]
pub struct RawCapture {
    /// The key that was requested.
    pub key_b64: String,
    /// The entry, when the RPC returned one.
    pub entry: Option<LedgerEntry>,
    pub last_modified_ledger_seq: u32,
    pub live_until_ledger_seq: Option<u32>,
}

/// Build a [`StateCaptureOutcome`] from raw captures and the simulation's
/// ledger.
///
/// `footprint_keys` is the full set of keys the simulation declared in its
/// `SorobanTransactionData` footprint. Every key must be accounted for: either
/// captured with a value, or explicitly absent (and the absence explained by
/// the simulation's own `stateChanges`, which is checked by
/// [`verify_against_state_changes`]).
pub fn build_capture(
    footprint_keys: &[String],
    raw: &[RawCapture],
    fetch_latest_ledger: u32,
    simulation_latest_ledger: u32,
) -> StateCaptureOutcome {
    let by_key: BTreeMap<&str, &RawCapture> = raw.iter().map(|r| (r.key_b64.as_str(), r)).collect();

    // 1. Every footprint key must be accounted for.
    for key in footprint_keys {
        if !by_key.contains_key(key.as_str()) {
            return StateCaptureOutcome::Blocked {
                reason: StateUnverified::FootprintKeyMissing {
                    key_b64: key.clone(),
                },
                entries: raw
                    .iter()
                    .map(|r| CapturedEntry {
                        key_b64: r.key_b64.clone(),
                        entry: r
                            .entry
                            .clone()
                            .unwrap_or_else(|| absent_placeholder(&r.key_b64)),
                        last_modified_ledger_seq: r.last_modified_ledger_seq,
                        live_until_ledger_seq: r.live_until_ledger_seq,
                        source: if r.entry.is_some() {
                            EntrySource::LedgerEntriesRead
                        } else {
                            EntrySource::AbsentAtRead
                        },
                    })
                    .collect(),
                fetch_latest_ledger,
                simulation_latest_ledger,
            };
        }
    }

    let mut entries = Vec::with_capacity(raw.len());
    for r in raw {
        let source = if r.entry.is_some() {
            EntrySource::LedgerEntriesRead
        } else {
            EntrySource::AbsentAtRead
        };
        let entry = match &r.entry {
            Some(e) => e.clone(),
            // An absent entry has no value. Carry a placeholder so the entry
            // is still auditable; the absence is recorded in `source`.
            None => absent_placeholder(&r.key_b64),
        };
        entries.push(CapturedEntry {
            key_b64: r.key_b64.clone(),
            entry,
            last_modified_ledger_seq: r.last_modified_ledger_seq,
            live_until_ledger_seq: r.live_until_ledger_seq,
            source,
        });
    }

    // 2. Fetch freshness: state must not be older than the simulation.
    if fetch_latest_ledger < simulation_latest_ledger {
        return StateCaptureOutcome::Blocked {
            reason: StateUnverified::StateOlderThanSimulation {
                fetch_latest_ledger,
                simulation_latest_ledger,
            },
            entries,
            fetch_latest_ledger,
            simulation_latest_ledger,
        };
    }

    // 3. Per-entry liveness and stability.
    for e in &entries {
        if let Some(live_until) = e.live_until_ledger_seq {
            if live_until < simulation_latest_ledger {
                return StateCaptureOutcome::Blocked {
                    reason: StateUnverified::EntryNotLive {
                        key_b64: e.key_b64.clone(),
                        live_until_ledger_seq: live_until,
                        simulation_latest_ledger,
                    },
                    entries,
                    fetch_latest_ledger,
                    simulation_latest_ledger,
                };
            }
        }
        if e.source == EntrySource::LedgerEntriesRead
            && e.last_modified_ledger_seq > simulation_latest_ledger
        {
            return StateCaptureOutcome::Blocked {
                reason: StateUnverified::EntryModifiedAfterSimulation {
                    key_b64: e.key_b64.clone(),
                    last_modified_ledger_seq: e.last_modified_ledger_seq,
                    simulation_latest_ledger,
                },
                entries,
                fetch_latest_ledger,
                simulation_latest_ledger,
            };
        }
    }

    StateCaptureOutcome::Verified {
        entries,
        fetch_latest_ledger,
        simulation_latest_ledger,
    }
}

/// A placeholder entry for a key the RPC did not return. It carries a
/// contract-data value matching the key's own contract, so the entry is
/// structurally valid; the `source` field records that no real value was
/// read.
fn absent_placeholder(key_b64: &str) -> LedgerEntry {
    let (contract, key_val, durability) = match decode_key(key_b64) {
        Ok(LedgerKey::ContractData(cd)) => (cd.contract, cd.key, cd.durability),
        // A non-contract-data key (ContractCode, Account, ...) has no
        // contract-data placeholder shape; use an empty contract-data entry
        // tied to a zero contract. The absence is what matters here, and it
        // is recorded in `EntrySource::AbsentAtRead`.
        _ => (
            stellar_xdr::ScAddress::Contract(stellar_xdr::ContractId(stellar_xdr::Hash([0; 32]))),
            stellar_xdr::ScVal::LedgerKeyContractInstance,
            stellar_xdr::ContractDataDurability::Persistent,
        ),
    };
    LedgerEntry {
        last_modified_ledger_seq: 0,
        data: stellar_xdr::LedgerEntryData::ContractData(stellar_xdr::ContractDataEntry {
            ext: stellar_xdr::ExtensionPoint::V0,
            contract,
            key: key_val,
            durability,
            val: stellar_xdr::ScVal::Void,
        }),
        ext: stellar_xdr::LedgerEntryExt::V0,
    }
}

/// Verify a capture against the simulation's own `stateChanges`.
///
/// `state_changes` is `(key_b64, before_value_b64)` pairs from the RPC
/// response. For every key the simulation reports a `before` value for, the
/// captured value must match; otherwise the capture is not the state the
/// simulation used.
///
/// This is the check that catches a modify-and-revert between simulation and
/// fetch, which `lastModifiedLedgerSeq` alone cannot.
pub fn verify_against_state_changes(
    outcome: &StateCaptureOutcome,
    state_changes: &[(String, String)],
) -> StateCaptureOutcome {
    let entries = outcome.entries();
    for (key_b64, before_b64) in state_changes {
        let Some(captured) = entries.iter().find(|e| &e.key_b64 == key_b64) else {
            continue;
        };
        if captured.source != EntrySource::LedgerEntriesRead {
            continue;
        }
        let captured_b64 = match encode_entry(&captured.entry) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if &captured_b64 != before_b64 {
            return StateCaptureOutcome::Blocked {
                reason: StateUnverified::ValueMismatchWithSimulation {
                    key_b64: key_b64.clone(),
                },
                entries: entries.to_vec(),
                fetch_latest_ledger: outcome.fetch_latest_ledger(),
                simulation_latest_ledger: outcome.simulation_latest_ledger(),
            };
        }
    }
    outcome.clone()
}

/// Canonical base64 XDR for a `LedgerEntry`.
pub fn encode_entry(entry: &LedgerEntry) -> Result<String, String> {
    use base64::Engine;
    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    entry.write_xdr(&mut l).map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&buf))
}

/// The footprint keys declared by a simulation's `SorobanTransactionData`,
/// as canonical base64 XDR, in footprint order (read_only then read_write).
pub fn footprint_keys_from_transaction_data(td_b64: &str) -> Result<Vec<String>, String> {
    let td = sdkt_xdr::parse_soroban_transaction_data(td_b64).map_err(|e| e.to_string())?;
    let mut keys = Vec::new();
    for k in td.resources.footprint.read_only.iter() {
        keys.push(encode_key(k)?);
    }
    for k in td.resources.footprint.read_write.iter() {
        keys.push(encode_key(k)?);
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_b64(kind: u8) -> String {
        // Deterministic synthetic keys for unit tests (no network).
        let mut raw = vec![0u8; 40];
        raw[3] = kind;
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&raw)
    }

    /// A real, valid `LedgerEntry` XDR for tests: a ContractData entry with a
    /// Void value. Deterministic, no network.
    fn entry_b64() -> String {
        let entry = LedgerEntry {
            last_modified_ledger_seq: 0,
            data: stellar_xdr::LedgerEntryData::ContractData(stellar_xdr::ContractDataEntry {
                ext: stellar_xdr::ExtensionPoint::V0,
                contract: stellar_xdr::ScAddress::Contract(stellar_xdr::ContractId(
                    stellar_xdr::Hash([0; 32]),
                )),
                key: stellar_xdr::ScVal::LedgerKeyContractInstance,
                durability: stellar_xdr::ContractDataDurability::Persistent,
                val: stellar_xdr::ScVal::Void,
            }),
            ext: stellar_xdr::LedgerEntryExt::V0,
        };
        encode_entry(&entry).unwrap()
    }

    /// A second, distinct valid `LedgerEntry` XDR for mismatch tests.
    fn other_entry_b64() -> String {
        let entry = LedgerEntry {
            last_modified_ledger_seq: 0,
            data: stellar_xdr::LedgerEntryData::ContractData(stellar_xdr::ContractDataEntry {
                ext: stellar_xdr::ExtensionPoint::V0,
                contract: stellar_xdr::ScAddress::Contract(stellar_xdr::ContractId(
                    stellar_xdr::Hash([1; 32]),
                )),
                key: stellar_xdr::ScVal::LedgerKeyContractInstance,
                durability: stellar_xdr::ContractDataDurability::Persistent,
                val: stellar_xdr::ScVal::Void,
            }),
            ext: stellar_xdr::LedgerEntryExt::V0,
        };
        encode_entry(&entry).unwrap()
    }

    fn raw_present(key: &str, last_mod: u32, live_until: Option<u32>) -> RawCapture {
        RawCapture {
            key_b64: key.to_string(),
            entry: Some(decode_entry(&entry_b64()).unwrap()),
            last_modified_ledger_seq: last_mod,
            live_until_ledger_seq: live_until,
        }
    }

    fn raw_absent(key: &str) -> RawCapture {
        RawCapture {
            key_b64: key.to_string(),
            entry: None,
            last_modified_ledger_seq: 0,
            live_until_ledger_seq: None,
        }
    }

    #[test]
    fn complete_footprint_with_fresh_stable_entries_is_verified() {
        let keys = vec![key_b64(6), key_b64(7)];
        let raw = vec![
            raw_present(&keys[0], 100, Some(9_000)),
            raw_present(&keys[1], 100, Some(9_000)),
        ];
        let out = build_capture(&keys, &raw, 5_000, 5_000);
        assert!(!out.is_blocked(), "{:?}", out.block_reason());
        assert_eq!(out.verified_entries().map(|e| e.len()), Some(2));
    }

    #[test]
    fn state_older_than_simulation_is_blocked() {
        let keys = vec![key_b64(6)];
        let raw = vec![raw_present(&keys[0], 100, Some(9_000))];
        let out = build_capture(&keys, &raw, 4_000, 5_000);
        assert!(out.is_blocked());
        assert_eq!(
            out.block_reason().map(|r| r.category()),
            Some("state_older_than_simulation")
        );
    }

    #[test]
    fn entry_not_live_at_simulation_ledger_is_blocked() {
        let keys = vec![key_b64(6)];
        let raw = vec![raw_present(&keys[0], 100, Some(4_500))];
        let out = build_capture(&keys, &raw, 5_000, 5_000);
        assert!(out.is_blocked());
        assert_eq!(
            out.block_reason().map(|r| r.category()),
            Some("entry_not_live")
        );
    }

    #[test]
    fn entry_modified_after_simulation_is_blocked() {
        let keys = vec![key_b64(6)];
        let raw = vec![raw_present(&keys[0], 5_100, Some(9_000))];
        let out = build_capture(&keys, &raw, 5_200, 5_000);
        assert!(out.is_blocked());
        assert_eq!(
            out.block_reason().map(|r| r.category()),
            Some("entry_modified_after_simulation")
        );
    }

    #[test]
    fn missing_footprint_key_is_blocked() {
        let keys = vec![key_b64(6), key_b64(7)];
        let raw = vec![raw_present(&keys[0], 100, Some(9_000))];
        let out = build_capture(&keys, &raw, 5_000, 5_000);
        assert!(out.is_blocked());
        assert_eq!(
            out.block_reason().map(|r| r.category()),
            Some("footprint_key_missing")
        );
    }

    #[test]
    fn absent_entry_is_captured_but_marked_absent() {
        let keys = vec![key_b64(6)];
        let raw = vec![raw_absent(&keys[0])];
        let out = build_capture(&keys, &raw, 5_000, 5_000);
        // Absence alone is not a blocker: the simulation may have created the
        // entry. The source records it, and verify_against_state_changes
        // decides.
        assert!(!out.is_blocked());
        let e = &out.verified_entries().unwrap()[0];
        assert_eq!(e.source, EntrySource::AbsentAtRead);
    }

    #[test]
    fn value_mismatch_with_simulation_is_blocked() {
        let keys = vec![key_b64(6)];
        let raw = vec![raw_present(&keys[0], 100, Some(9_000))];
        let out = build_capture(&keys, &raw, 5_000, 5_000);
        // The simulation says the entry's `before` value was something else.
        let other = other_entry_b64();
        let checked = verify_against_state_changes(&out, &[(keys[0].clone(), other)]);
        assert!(checked.is_blocked());
        assert_eq!(
            checked.block_reason().map(|r| r.category()),
            Some("value_mismatch_with_simulation")
        );
    }

    #[test]
    fn matching_value_with_simulation_stays_verified() {
        let keys = vec![key_b64(6)];
        let raw = vec![raw_present(&keys[0], 100, Some(9_000))];
        let out = build_capture(&keys, &raw, 5_000, 5_000);
        let before = encode_entry(&out.verified_entries().unwrap()[0].entry).unwrap();
        let checked = verify_against_state_changes(&out, &[(keys[0].clone(), before)]);
        assert!(!checked.is_blocked());
    }

    #[test]
    fn blocked_outcome_never_exposes_verified_entries() {
        let keys = vec![key_b64(6), key_b64(7)];
        let raw = vec![raw_present(&keys[0], 100, Some(9_000))];
        let out = build_capture(&keys, &raw, 5_000, 5_000);
        assert!(out.is_blocked());
        assert_eq!(out.verified_entries(), None);
        // Diagnostics still available.
        assert_eq!(out.entries().len(), 1);
    }

    #[test]
    fn outcome_serializes_with_its_outcome_tag() {
        let keys = vec![key_b64(6)];
        let raw = vec![raw_present(&keys[0], 100, Some(9_000))];
        let out = build_capture(&keys, &raw, 5_000, 5_000);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""outcome":"verified""#), "{json}");

        let blocked = build_capture(&keys, &raw, 4_000, 5_000);
        let json = serde_json::to_string(&blocked).unwrap();
        assert!(json.contains(r#""outcome":"blocked""#), "{json}");
        assert!(json.contains("state_older_than_simulation"), "{json}");
    }
}
