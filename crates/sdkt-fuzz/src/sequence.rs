//! Deterministic stateful sequences.
//!
//! A sequence is an ordered list of calls executed so that **state carries
//! forward between steps within one sequence**, while every step runs
//! against a *fresh* Host seeded from the accumulated baseline.
//!
//! Architecture (no `Host::clone` snapshots anywhere):
//!
//! ```text
//! state₀ ──► step 1 (fresh Host, shared ModuleCache)
//!             │  observation.state (footprint diff)
//!             ▼
//!           state₁ = data entries ⊕ instance storage
//!             │
//!             ▼
//!           step 2 (fresh Host) ──► … ──► final Observation
//! ```
//!
//! Isolation: sequences are built from a caller-supplied baseline, never
//! from ambient state, so an `A / B / A` sequence sees `B`'s effect on `A`
//! only *within* the sequence, and never in the next sequence.

use soroban_env_host::xdr::{LedgerEntry, LedgerEntryData, LedgerKey, Limits, ReadXdr, ScVal};

use crate::case::FunctionCall;
use crate::environment::Environment;
use crate::error::FuzzError;
use crate::executor::Executor;
use crate::observation::{Observation, StateChange};

/// The state a sequence carries forward between steps.
///
/// The instance entry itself is executor-owned (it writes the executable
/// pointer per execution), so instance state is carried as its storage map,
/// and ledger-level state as plain contract-data entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SequenceState {
    /// Contract-data entries (instance entries excluded — the executor
    /// rejects them in a case's `baseline_entries`).
    pub data_entries: Vec<LedgerEntry>,
    /// Instance-storage pairs, applied via
    /// [`crate::FuzzCase::with_instance_storage`].
    pub instance_storage: Vec<(ScVal, ScVal)>,
}

impl SequenceState {
    /// Split a caller-supplied baseline into carried data + instance storage.
    pub fn from_baseline(baseline: &[LedgerEntry]) -> Result<Self, FuzzError> {
        let mut data_entries = Vec::new();
        let mut instance_storage = Vec::new();
        for entry in baseline {
            match &entry.data {
                LedgerEntryData::ContractData(d) => {
                    if matches!(d.key, ScVal::LedgerKeyContractInstance) {
                        if let ScVal::ContractInstance(inst) = &d.val {
                            instance_storage = read_storage_map(inst.storage.as_ref())?;
                        }
                    } else {
                        data_entries.push(entry.clone());
                    }
                }
                LedgerEntryData::ContractCode(_) => {
                    return Err(FuzzError::InvalidSetup(
                        crate::error::SetupError::BaselineEntry(
                            "sequence baselines carry contract-data entries only".to_string(),
                        ),
                    ));
                }
                _ => {
                    return Err(FuzzError::InvalidSetup(
                        crate::error::SetupError::BaselineEntry(
                            "only LedgerEntryData::ContractData sequence entries are supported"
                                .to_string(),
                        ),
                    ));
                }
            }
        }
        Ok(Self {
            data_entries,
            instance_storage,
        })
    }
}

/// Read an instance storage map into a deterministic key/value list.
pub(crate) fn read_storage_map(
    map: Option<&soroban_env_host::xdr::ScMap>,
) -> Result<Vec<(ScVal, ScVal)>, FuzzError> {
    Ok(map
        .map(|m| {
            m.0.iter()
                .map(|e| (e.key.clone(), e.val.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default())
}

/// One step's result inside a sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepObservation {
    pub step_index: usize,
    pub function: String,
    pub observation: Observation,
}

/// A completed sequence run: every step's observation, plus the final one.
///
/// `final_observation` is what the oracle classifies; for a single-step
/// sequence it equals the single-step path's observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceRun {
    pub case_id: String,
    pub steps: Vec<StepObservation>,
    pub final_observation: Observation,
    /// The full carried state after the final step.
    pub state: SequenceState,
}

impl SequenceRun {
    /// State readback: changed entries after the final step.
    pub fn changes(&self) -> impl Iterator<Item = &crate::observation::StateEntry> {
        self.final_observation.changes()
    }
}

/// Execute `steps` in order, carrying state forward between steps.
///
/// Contract-level failures are observations (single-step semantics); a
/// `FuzzError` means the sequence could not be executed at all.
pub fn execute_sequence(
    executor: &Executor,
    baseline: &[LedgerEntry],
    environment: &Environment,
    steps: &[FunctionCall],
    case_id: &str,
) -> Result<SequenceRun, FuzzError> {
    execute_sequence_auth(executor, baseline, environment, steps, case_id, &[])
}

/// Sequence execution with per-step authorization entries.
///
/// `auth_per_step[i]` holds pre-encoded auth entries for step `i` (see
/// [`crate::auth`]). A missing or empty entry list means `NoAuth` for that
/// step — the host enforces against an empty set. Each step is a separate
/// host invocation, so per-step roots mirror the empirically verified
/// single-call semantics; no auth-tree semantics are claimed.
pub fn execute_sequence_auth(
    executor: &Executor,
    baseline: &[LedgerEntry],
    environment: &Environment,
    steps: &[FunctionCall],
    case_id: &str,
    auth_per_step: &[Vec<Vec<u8>>],
) -> Result<SequenceRun, FuzzError> {
    if steps.is_empty() {
        return Err(FuzzError::InvalidConfig(
            "sequence must contain at least one step".to_string(),
        ));
    }

    let mut state = SequenceState::from_baseline(baseline)?;
    let mut observed = Vec::with_capacity(steps.len());
    let mut final_observation: Option<Observation> = None;

    for (i, call) in steps.iter().enumerate() {
        let step_case = format!("{case_id}/step{i}");
        let case = executor
            .case(step_case, call.clone(), state.data_entries.clone())
            .with_instance_storage(state.instance_storage.clone());
        let obs = if let Some(entries) = auth_per_step.get(i) {
            executor.execute_with_auth(&case, environment, entries)?
        } else {
            executor.execute_with(&case, environment)?
        };

        state = apply_state_delta(state, &obs)?;
        observed.push(StepObservation {
            step_index: i,
            function: call.function.clone(),
            observation: obs.clone(),
        });
        final_observation = Some(obs);
    }

    Ok(SequenceRun {
        case_id: case_id.to_string(),
        steps: observed,
        final_observation: final_observation.expect("non-empty steps yield an observation"),
        state,
    })
}

/// Apply an observation's footprint diff onto carried state.
///
/// Executor-owned entries (contract code, instance entry key, nonce entries)
/// are folded specially: contract-data entries are replaced by value, and
/// the instance entry's *storage map* is carried as
/// [`SequenceState::instance_storage`]. Nonce entries are host-generated
/// temporaries and are never contract storage, so they are skipped — the same
/// distinction [`crate::state_capture::is_executor_owned_key`] makes.
pub fn apply_state_delta(
    state: SequenceState,
    obs: &Observation,
) -> Result<SequenceState, FuzzError> {
    let mut out = state;
    for entry in &obs.state {
        if entry.change == StateChange::Unchanged {
            continue;
        }
        let key = LedgerKey::from_xdr(entry.key_xdr.as_slice(), Limits::none())
            .map_err(|e| FuzzError::Observation(format!("state key did not decode: {e}")))?;
        match &key {
            LedgerKey::ContractCode(_) => continue, // executor-owned
            LedgerKey::ContractData(d) if matches!(d.key, ScVal::LedgerKeyContractInstance) => {
                if let Some(value_xdr) = &entry.value_xdr {
                    let updated = decode_entry(value_xdr)?;
                    if let LedgerEntryData::ContractData(new_d) = &updated.data {
                        if let ScVal::ContractInstance(inst) = &new_d.val {
                            out.instance_storage = read_storage_map(inst.storage.as_ref())?;
                        }
                    }
                } else {
                    out.instance_storage.clear();
                }
            }
            LedgerKey::ContractData(d) if matches!(d.key, ScVal::LedgerKeyNonce(_)) => {
                continue; // executor-owned: host-generated temporary, never contract storage
            }
            LedgerKey::ContractData(_) => {
                out.data_entries.retain(|e| e.to_key() != key);
                if let Some(value_xdr) = &entry.value_xdr {
                    out.data_entries.push(decode_entry(value_xdr)?);
                }
            }
            _ => continue,
        }
    }
    Ok(out)
}

fn decode_entry(bytes: &[u8]) -> Result<LedgerEntry, FuzzError> {
    LedgerEntry::from_xdr(bytes, Limits::none())
        .map_err(|e| FuzzError::Observation(format!("state value did not decode: {e}")))
}

/// Why RPC `stateChanges` could not become the next invocation's baseline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainError {
    /// A decoded row's key is not a `ContractData` key the sequence model
    /// can carry (e.g. an `Account` or `Ttl` key the RPC reported).
    UnsupportedKey { key_b64: String },
    /// A decoded row's entry is not `ContractData` (e.g. a `ContractCode`
    /// entry the RPC reported as created).
    UnsupportedEntry { key_b64: String },
    /// The row's `after` value does not decode to the key the row reports.
    KeyMismatch { key_b64: String },
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainError::UnsupportedKey { key_b64 } => write!(
                f,
                "stateChanges row key {key_b64} is not contract-data state the sequence can carry"
            ),
            ChainError::UnsupportedEntry { key_b64 } => write!(
                f,
                "stateChanges row {key_b64} is not a contract-data entry the sequence can carry"
            ),
            ChainError::KeyMismatch { key_b64 } => write!(
                f,
                "stateChanges row {key_b64} decodes to a different key than reported"
            ),
        }
    }
}

/// Build the next invocation's [`SequenceState`] from decoded RPC
/// `stateChanges` rows, applied onto an existing baseline.
///
/// This is the chaining primitive: the RPC's post-execution `after` values
/// become the next invocation's pre-execution state.
///
/// Semantics, per row kind (for non-executor-owned `ContractData` rows):
/// - `Created` / `Updated` — the `after` entry replaces any baseline entry
///   with the same key;
/// - `Deleted` — the key is removed from the baseline;
/// - executor-owned rows (contract code, instance singleton, nonce) are
///   **skipped**, consistently with [`crate::state_capture::is_executor_owned_key`]
///   and [`crate::state_compare`]: a nonce is host bookkeeping, the
///   instance singleton is executor-owned (its storage map is carried only
///   from the local baseline via `with_instance_storage`, never from RPC
///   rows), and code is resolved by the executor, not carried as storage.
///   Skipping rather than folding is deliberate: an RPC `after` for the
///   instance singleton would silently overwrite the local baseline's
///   instance storage with network state, breaking the isolation the
///   sequence model guarantees.
///
/// What is *not* carried (and why):
/// - `lastModifiedLedgerSeq`, `liveUntilLedgerSeq`, TTL — the RPC's
///   `stateChanges` rows do not report them. The executor seeds its own
///   ledger metadata for local execution; that metadata is synthetic and is
///   never presented as observed network state.
/// - keys or entries that are not `ContractData` — returned as
///   [`ChainError`], never silently dropped.
///
/// `Unchanged` rows carry no `after` and change nothing; they are accepted
/// and ignored.
pub fn state_changes_into_sequence_state(
    baseline: &SequenceState,
    changes: &[crate::state_capture::DecodedStateChange],
) -> Result<SequenceState, ChainError> {
    let mut out = baseline.clone();
    for change in changes {
        let key = crate::state_capture::decode_key(&change.key_b64).map_err(|_| {
            ChainError::UnsupportedKey {
                key_b64: change.key_b64.clone(),
            }
        })?;
        if crate::state_capture::is_executor_owned_key(&key) {
            continue;
        }
        let LedgerKey::ContractData(_) = &key else {
            return Err(ChainError::UnsupportedKey {
                key_b64: change.key_b64.clone(),
            });
        };
        match change.kind {
            crate::state_capture::StateChangeKind::Created
            | crate::state_capture::StateChangeKind::Updated => {
                let after = change.after.as_ref().ok_or(ChainError::UnsupportedEntry {
                    key_b64: change.key_b64.clone(),
                })?;
                if after.to_key() != key {
                    return Err(ChainError::KeyMismatch {
                        key_b64: change.key_b64.clone(),
                    });
                }
                let LedgerEntryData::ContractData(_) = &after.data else {
                    return Err(ChainError::UnsupportedEntry {
                        key_b64: change.key_b64.clone(),
                    });
                };
                out.data_entries.retain(|e| e.to_key() != key);
                out.data_entries.push(after.clone());
            }
            crate::state_capture::StateChangeKind::Deleted => {
                out.data_entries.retain(|e| e.to_key() != key);
            }
        }
    }
    Ok(out)
}
