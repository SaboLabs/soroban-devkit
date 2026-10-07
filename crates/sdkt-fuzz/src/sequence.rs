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
/// Executor-owned entries (contract code, instance entry key) are folded
/// specially: contract-data entries are replaced by value, and the instance
/// entry's *storage map* is carried as [`SequenceState::instance_storage`].
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
