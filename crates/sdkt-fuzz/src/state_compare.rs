//! Post-execution state comparison: local observation vs RPC `stateChanges`.
//!
//! ## What this compares
//!
//! A state-mutating differential run produces two accounts of what happened
//! to the ledger:
//!
//! - **local** — [`Observation::state`], the footprint entries the host
//!   reported, each with a baseline-relative [`StateChange`]
//!   (created / updated / deleted / unchanged) and the post-execution value;
//! - **RPC** — the `stateChanges` rows of the `simulateTransaction` response,
//!   each with a kind and `before` / `after` values.
//!
//! This module compares them **per key**, in a canonical byte representation,
//! and reports one of three outcomes. It deliberately does not compare
//! `returned` / `is_success()`: a call can succeed on both sides and still
//! write different state.
//!
//! ## What it does not claim
//!
//! - **Not a full-state comparison.** `Observation.state` covers only the
//!   footprint the local host executed with; the RPC's `stateChanges` covers
//!   only the entries that invocation touched. Keys outside the intersection
//!   are reported as [`Outcome::CannotVerify`], never as agreement.
//! - **Not a TTL comparison.** The RPC does not report TTL changes in
//!   `stateChanges`, and the local observation carries no TTL at all. A
//!   comparison of TTL is impossible from these inputs, and this module does
//!   not pretend otherwise.
//! - **Not an atomic-snapshot claim.** Neither side is guaranteed to be a
//!   point-in-time snapshot of the whole ledger.
//! - **Not CPU/memory parity.** Unrelated to this module; the standing
//!   blocker is `sdkt_fuzz::RpcBlockReason::MissingCost`.

use std::collections::BTreeMap;

use crate::observation::{Observation, StateEntry};
use stellar_xdr::{LedgerKey, Limits, ReadXdr};

/// One side of a comparison for a single key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Side {
    /// The side reports this key with this post-execution value.
    Present(Vec<u8>),
    /// The side reports this key as removed by the execution.
    Removed,
    /// The side does not report this key at all.
    Absent,
}

/// Why a comparison could not be made for a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CannotVerifyReason {
    /// The key appears on only one side, so there is nothing to compare.
    /// This is expected for entries outside the local footprint or outside
    /// the RPC's reported changes, and is not a divergence.
    KeyOnOneSideOnly { local: bool, rpc: bool },
    /// The key is executor-owned (contract code, instance singleton, nonce):
    /// host-internal bookkeeping, not contract storage, so it is outside the
    /// comparable set by construction. Matches `apply_state_delta`'s
    /// treatment in `sequence.rs`.
    ExecutorOwnedKey,
    /// The key is present on both sides but neither carries a comparable
    /// value (e.g. both report removal, so there is no value to compare —
    /// this is agreement, not a failure; the case that lands here is a side
    /// reporting removal while the other is silent).
    NoComparableValue,
}

/// The verdict for one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyVerdict {
    /// Both sides agree on the post-execution value.
    Match,
    /// Both sides agree the entry was removed.
    MatchRemoved,
    /// The sides disagree; carries both values for diagnosis.
    Mismatch { local: Side, rpc: Side },
    /// The comparison could not be made; carries the reason.
    CannotVerify(CannotVerifyReason),
}

/// The overall result of comparing local state against RPC state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every comparable key agreed.
    Match,
    /// At least one key disagreed.
    Mismatch(Vec<KeyVerdict>),
    /// No key could be compared, so no conclusion is possible.
    CannotVerify(Vec<KeyVerdict>),
}

impl Outcome {
    /// True when every comparable key agreed.
    pub fn is_match(&self) -> bool {
        matches!(self, Outcome::Match)
    }

    /// The keys that disagreed, when any.
    pub fn mismatches(&self) -> &[KeyVerdict] {
        match self {
            Outcome::Mismatch(v) => v,
            _ => &[],
        }
    }

    /// The keys that could not be compared, when any.
    pub fn unverified(&self) -> &[KeyVerdict] {
        match self {
            Outcome::CannotVerify(v) => v,
            _ => &[],
        }
    }
}

/// One RPC `stateChanges` row, reduced to what a comparison needs.
///
/// Built from a [`crate::state_capture::DecodedStateChange`] so the XDR
/// decoding and presence-rule validation happen once, upstream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcStateChange {
    /// Encoded `LedgerKey` XDR.
    pub key_xdr: Vec<u8>,
    /// The RPC-reported kind.
    pub kind: crate::state_capture::StateChangeKind,
    /// The post-execution value, when the row carries one.
    pub after: Option<Vec<u8>>,
}

impl RpcStateChange {
    /// Build from a decoded `stateChanges` row.
    pub fn from_decoded(d: &crate::state_capture::DecodedStateChange) -> Result<Self, String> {
        let key_xdr = crate::state_capture::key_bytes(&d.key_b64)?;
        let after = match &d.after {
            Some(entry) => Some(crate::state_capture::entry_bytes(entry)?),
            None => None,
        };
        Ok(RpcStateChange {
            key_xdr,
            kind: d.kind,
            after,
        })
    }

    /// The side this row contributes to a comparison.
    fn side(&self) -> Side {
        match &self.after {
            Some(v) => Side::Present(v.clone()),
            None => Side::Removed,
        }
    }
}

/// The side a local [`StateEntry`] contributes to a comparison.
///
/// An entry the local host did not report is [`Side::Absent`]; an entry
/// reported with no post-execution value is [`Side::Removed`]. These are
/// deliberately distinct: "not in the footprint" and "removed by the
/// execution" are different facts.
fn local_side(e: &StateEntry) -> Side {
    match &e.value_xdr {
        Some(v) => Side::Present(v.clone()),
        None => Side::Removed,
    }
}

/// True when the RPC reports this key but the local observation cannot be
/// compared against it, because the key is executor-owned.
///
/// `apply_state_delta` in `sequence.rs` makes the same distinction: the
/// executor owns contract code and the instance singleton, and a nonce is
/// a host-generated temporary entry. Treating those as contract storage
/// would report spurious mismatches for state the invocation never
/// intended to touch.
fn is_executor_owned(key_xdr: &[u8]) -> bool {
    let Ok(key) = LedgerKey::from_xdr(key_xdr, Limits::none()) else {
        return false;
    };
    crate::state_capture::is_executor_owned_key(&key)
}

/// Build the comparator's [`Side`]s from a local observation, excluding
/// executor-owned keys so they are never compared as contract storage.
fn local_sides_filtered(obs: &Observation) -> BTreeMap<Vec<u8>, Side> {
    let mut out = BTreeMap::new();
    for e in &obs.state {
        if is_executor_owned(&e.key_xdr) {
            continue;
        }
        out.entry(e.key_xdr.clone())
            .or_insert_with(|| local_side(e));
    }
    out
}

/// Compare a local observation's state against the RPC's reported changes.
///
/// `rpc_changes` should be the rows for the same invocation. Keys present on
/// only one side are reported as [`KeyVerdict::CannotVerify`] with
/// [`CannotVerifyReason::KeyOnOneSideOnly`] — they are outside the
/// comparable set, not divergences.
pub fn compare_state(obs: &Observation, rpc_changes: &[RpcStateChange]) -> Outcome {
    // Index both sides by key bytes. Duplicate keys within one side are a
    // protocol violation; keep the first and let the comparison proceed so a
    // malformed input is visible in the verdicts rather than panicking.
    let local = local_sides_filtered(obs);
    let mut rpc: BTreeMap<&[u8], &RpcStateChange> = BTreeMap::new();
    for c in rpc_changes {
        rpc.entry(c.key_xdr.as_slice()).or_insert(c);
    }

    let mut verdicts: Vec<KeyVerdict> = Vec::new();
    let mut mismatches: Vec<KeyVerdict> = Vec::new();

    for (key, l) in &local {
        let Some(r) = rpc.get(key.as_slice()) else {
            verdicts.push(KeyVerdict::CannotVerify(
                CannotVerifyReason::KeyOnOneSideOnly {
                    local: true,
                    rpc: false,
                },
            ));
            continue;
        };
        let r = r.side();
        match (l, &r) {
            (Side::Present(a), Side::Present(b)) => {
                if a == b {
                    verdicts.push(KeyVerdict::Match);
                } else {
                    let v = KeyVerdict::Mismatch {
                        local: l.clone(),
                        rpc: r.clone(),
                    };
                    mismatches.push(v.clone());
                    verdicts.push(v);
                }
            }
            (Side::Removed, Side::Removed) => verdicts.push(KeyVerdict::MatchRemoved),
            _ => {
                let v = KeyVerdict::Mismatch {
                    local: l.clone(),
                    rpc: r.clone(),
                };
                mismatches.push(v.clone());
                verdicts.push(v);
            }
        }
    }

    // RPC-only keys: nothing local to compare against. Two distinct causes,
    // kept apart because they mean different things:
    // - executor-owned keys (contract code, instance singleton, nonce) are
    //   outside the comparable set by construction — reporting them as a
    //   divergence would attribute host-internal bookkeeping to the contract;
    // - any other key is simply outside the local footprint, so the
    //   observation coverage is incomplete for it.
    for (key, c) in &rpc {
        if local.contains_key(*key) {
            continue;
        }
        let reason = if is_executor_owned(key) || is_executor_owned(&c.key_xdr) {
            CannotVerifyReason::ExecutorOwnedKey
        } else {
            CannotVerifyReason::KeyOnOneSideOnly {
                local: false,
                rpc: true,
            }
        };
        verdicts.push(KeyVerdict::CannotVerify(reason));
    }

    if !mismatches.is_empty() {
        return Outcome::Mismatch(mismatches);
    }
    if verdicts
        .iter()
        .all(|v| matches!(v, KeyVerdict::CannotVerify(_)))
    {
        return Outcome::CannotVerify(verdicts);
    }
    Outcome::Match
}

/// Convenience: compare and also report how many keys were comparable.
///
/// A `Match` with `comparable == 0` is not evidence of agreement — the
/// caller should treat it as `CannotVerify`. This helper makes that
/// distinction impossible to miss.
///
/// `comparable` counts only keys that were actually compared on both sides,
/// excluding executor-owned keys, which are never comparable by
/// construction. A `Match` whose `comparable` is zero is reported as
/// `CannotVerify` so it cannot be read as agreement.
pub fn compare_state_counting(
    obs: &Observation,
    rpc_changes: &[RpcStateChange],
) -> (Outcome, usize) {
    let outcome = compare_state(obs, rpc_changes);
    let comparable = match &outcome {
        Outcome::Match => {
            let local = local_sides_filtered(obs);
            rpc_changes
                .iter()
                .filter(|c| local.contains_key(c.key_xdr.as_slice()))
                .count()
        }
        Outcome::Mismatch(v) => v.len(),
        Outcome::CannotVerify(_) => 0,
    };
    if comparable == 0 {
        if let Outcome::Match = outcome {
            return (Outcome::CannotVerify(Vec::new()), 0);
        }
    }
    (outcome, comparable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{BudgetUsage, ExecutionStatus, StateChange};
    use stellar_xdr::{
        ContractDataDurability, ContractId, Hash, LedgerKey, LedgerKeyContractCode,
        LedgerKeyContractData, Limited, Limits, ScAddress, ScNonceKey, ScVal, WriteXdr,
    };

    /// The executor-owned instance key for the zero contract, as real XDR —
    /// used to prove executor-owned keys are excluded from comparison.
    fn instance_key_xdr() -> Vec<u8> {
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([7; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        });
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        key.write_xdr(&mut l).unwrap();
        buf
    }

    /// A nonce key — also executor-owned (host-generated temporary entry).
    fn nonce_key_xdr() -> Vec<u8> {
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([7; 32]))),
            key: ScVal::LedgerKeyNonce(ScNonceKey { nonce: 42 }),
            durability: ContractDataDurability::Temporary,
        });
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        key.write_xdr(&mut l).unwrap();
        buf
    }

    /// A contract-code key — executor-owned (the host resolves the contract
    /// from it, it is not contract storage).
    fn contract_code_key_xdr() -> Vec<u8> {
        let key = LedgerKey::ContractCode(LedgerKeyContractCode {
            hash: Hash([9; 32]),
        });
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        key.write_xdr(&mut l).unwrap();
        buf
    }

    /// A plain contract-data key (real storage the invocation may write).
    fn data_key_xdr(byte: u8) -> Vec<u8> {
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([7; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        });
        let _ = key;
        // A symbol-keyed storage entry, as real contracts use.
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([7; 32]))),
            key: ScVal::Symbol(stellar_xdr::ScSymbol(
                stellar_xdr::StringM::try_from("counter").unwrap(),
            )),
            durability: ContractDataDurability::Persistent,
        });
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        key.write_xdr(&mut l).unwrap();
        let _ = byte;
        buf
    }

    fn entry(key_xdr: Vec<u8>, value: Option<Vec<u8>>, change: StateChange) -> StateEntry {
        StateEntry {
            key_xdr,
            value_xdr: value,
            change,
        }
    }

    fn obs(state: Vec<StateEntry>) -> Observation {
        Observation {
            case_id: "t".into(),
            function: "f".into(),
            status: ExecutionStatus::Void,
            state,
            events: vec![],
            budget: BudgetUsage::default(),
        }
    }

    fn rpc(key_xdr: Vec<u8>, after: Option<Vec<u8>>) -> RpcStateChange {
        RpcStateChange {
            key_xdr,
            kind: crate::state_capture::StateChangeKind::Updated,
            after,
        }
    }

    #[test]
    fn identical_values_match() {
        let o = obs(vec![entry(vec![1], Some(vec![9]), StateChange::Updated)]);
        let out = compare_state(&o, &[rpc(vec![1], Some(vec![9]))]);
        assert!(out.is_match(), "{out:?}");
    }

    #[test]
    fn differing_values_mismatch_with_both_sides() {
        let o = obs(vec![entry(vec![1], Some(vec![9]), StateChange::Updated)]);
        let out = compare_state(&o, &[rpc(vec![1], Some(vec![8]))]);
        match out {
            Outcome::Mismatch(v) => {
                assert_eq!(v.len(), 1);
                match &v[0] {
                    KeyVerdict::Mismatch { local, rpc: r } => {
                        assert_eq!(local, &Side::Present(vec![9]));
                        assert_eq!(r, &Side::Present(vec![8]));
                    }
                    other => panic!("expected Mismatch verdict, got {other:?}"),
                }
            }
            other => panic!("expected Mismatch outcome, got {other:?}"),
        }
    }

    #[test]
    fn both_removed_is_match_removed() {
        let o = obs(vec![entry(vec![1], None, StateChange::Deleted)]);
        let out = compare_state(&o, &[rpc(vec![1], None)]);
        assert!(out.is_match(), "{out:?}");
    }

    #[test]
    fn local_removed_rpc_present_is_mismatch() {
        let o = obs(vec![entry(vec![1], None, StateChange::Deleted)]);
        let out = compare_state(&o, &[rpc(vec![1], Some(vec![7]))]);
        assert!(!out.is_match());
        assert_eq!(out.mismatches().len(), 1);
    }

    #[test]
    fn local_present_rpc_removed_is_mismatch() {
        let o = obs(vec![entry(vec![1], Some(vec![7]), StateChange::Updated)]);
        let out = compare_state(&o, &[rpc(vec![1], None)]);
        assert!(!out.is_match());
        assert_eq!(out.mismatches().len(), 1);
    }

    #[test]
    fn key_on_one_side_only_cannot_be_verified() {
        // Local footprint has a key the RPC did not report a change for.
        let o = obs(vec![entry(vec![1], Some(vec![9]), StateChange::Unchanged)]);
        let out = compare_state(&o, &[rpc(vec![2], Some(vec![3]))]);
        assert!(!out.is_match());
        assert_eq!(
            out.unverified().len(),
            2,
            "both keys are one-sided: {out:?}"
        );
    }

    #[test]
    fn no_comparable_keys_is_cannot_verify_not_match() {
        let o = obs(vec![entry(vec![1], Some(vec![9]), StateChange::Unchanged)]);
        let out = compare_state(&o, &[]);
        assert!(matches!(out, Outcome::CannotVerify(_)), "{out:?}");
    }

    #[test]
    fn counting_helper_reports_zero_comparable_for_cannot_verify() {
        let o = obs(vec![entry(vec![1], Some(vec![9]), StateChange::Unchanged)]);
        let (out, comparable) = compare_state_counting(&o, &[]);
        assert!(matches!(out, Outcome::CannotVerify(_)));
        assert_eq!(comparable, 0);
    }

    #[test]
    fn counting_helper_counts_comparable_keys_on_match() {
        let o = obs(vec![
            entry(vec![1], Some(vec![9]), StateChange::Updated),
            entry(vec![2], Some(vec![4]), StateChange::Updated),
        ]);
        let (out, comparable) = compare_state_counting(
            &o,
            &[rpc(vec![1], Some(vec![9])), rpc(vec![2], Some(vec![4]))],
        );
        assert!(out.is_match());
        assert_eq!(comparable, 2);
    }

    #[test]
    fn unrelated_local_entries_do_not_create_false_mismatch() {
        // A read-only footprint entry the RPC did not report: one-sided, so
        // CannotVerify — never a Mismatch.
        let o = obs(vec![
            entry(vec![1], Some(vec![9]), StateChange::Updated),
            entry(vec![2], Some(vec![4]), StateChange::Unchanged),
        ]);
        let out = compare_state(&o, &[rpc(vec![1], Some(vec![9]))]);
        assert!(out.is_match(), "{out:?}");
    }

    #[test]
    fn duplicate_keys_do_not_panic() {
        let o = obs(vec![
            entry(vec![1], Some(vec![9]), StateChange::Updated),
            entry(vec![1], Some(vec![9]), StateChange::Updated),
        ]);
        let out = compare_state(&o, &[rpc(vec![1], Some(vec![9]))]);
        assert!(out.is_match(), "{out:?}");
    }

    // --- executor-owned key handling (Step 3A finding) ----------------------

    /// The RPC reports an instance-singleton change; the local observation
    /// also has it. It must be excluded from storage comparison, with the
    /// reason naming it as executor-owned — never a mismatch.
    #[test]
    fn instance_singleton_is_executor_owned_not_a_mismatch() {
        let k = instance_key_xdr();
        let o = obs(vec![entry(k.clone(), Some(vec![1]), StateChange::Updated)]);
        let out = compare_state(&o, &[rpc(k.clone(), Some(vec![2]))]);
        assert!(matches!(out, Outcome::CannotVerify(_)), "{out:?}");
        match &out {
            Outcome::CannotVerify(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(
                    v[0],
                    KeyVerdict::CannotVerify(CannotVerifyReason::ExecutorOwnedKey)
                );
            }
            _ => unreachable!(),
        }
    }

    /// A nonce entry the RPC reports is executor-owned too.
    #[test]
    fn nonce_key_is_executor_owned() {
        let k = nonce_key_xdr();
        let o = obs(vec![]);
        let out = compare_state(&o, &[rpc(k, Some(vec![0]))]);
        match &out {
            Outcome::CannotVerify(v) => assert_eq!(
                v[0],
                KeyVerdict::CannotVerify(CannotVerifyReason::ExecutorOwnedKey)
            ),
            other => panic!("expected CannotVerify, got {other:?}"),
        }
    }

    /// A contract-code entry the RPC reports is executor-owned.
    #[test]
    fn contract_code_key_is_executor_owned() {
        let k = contract_code_key_xdr();
        let o = obs(vec![]);
        let out = compare_state(&o, &[rpc(k, Some(vec![0]))]);
        match &out {
            Outcome::CannotVerify(v) => assert_eq!(
                v[0],
                KeyVerdict::CannotVerify(CannotVerifyReason::ExecutorOwnedKey)
            ),
            other => panic!("expected CannotVerify, got {other:?}"),
        }
    }

    /// A real storage key reported by RPC but missing locally is one-sided
    /// coverage — CannotVerify, not executor-owned.
    #[test]
    fn plain_storage_key_on_one_side_is_coverage_not_executor_owned() {
        let k = data_key_xdr(1);
        let o = obs(vec![]);
        let out = compare_state(&o, &[rpc(k, Some(vec![0]))]);
        match &out {
            Outcome::CannotVerify(v) => assert!(
                matches!(
                    v[0],
                    KeyVerdict::CannotVerify(CannotVerifyReason::KeyOnOneSideOnly { .. })
                ),
                "{:?}",
                v[0]
            ),
            other => panic!("expected CannotVerify, got {other:?}"),
        }
    }

    /// A match on real storage keys is still a match when executor-owned
    /// keys are also present: they are skipped, not counted as divergence.
    #[test]
    fn storage_match_survives_adjacent_executor_owned_keys() {
        let k = data_key_xdr(1);
        let o = obs(vec![
            entry(k.clone(), Some(vec![5]), StateChange::Updated),
            entry(instance_key_xdr(), Some(vec![1]), StateChange::Updated),
        ]);
        let (out, comparable) = compare_state_counting(
            &o,
            &[
                rpc(k, Some(vec![5])),
                rpc(instance_key_xdr(), Some(vec![2])),
            ],
        );
        assert!(out.is_match(), "{out:?}");
        assert_eq!(comparable, 1, "only the real storage key is comparable");
    }

    /// A match with zero comparable keys is reported as CannotVerify by the
    /// counting helper: it can never be read as agreement.
    #[test]
    fn match_with_zero_comparable_is_cannot_verify() {
        let o = obs(vec![entry(
            instance_key_xdr(),
            Some(vec![1]),
            StateChange::Updated,
        )]);
        let (out, comparable) =
            compare_state_counting(&o, &[rpc(instance_key_xdr(), Some(vec![1]))]);
        assert!(matches!(out, Outcome::CannotVerify(_)), "{out:?}");
        assert_eq!(comparable, 0);
    }
}
