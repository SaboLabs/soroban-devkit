//! Execution observation — the SDKT-owned result model.
//!
//! Nothing from `soroban-env-host`'s result types leaks past this module:
//! the host's `InvokeHostFunctionResult` is mapped into [`Observation`] here
//! so the rest of SDKT depends on a stable, SDKT-shaped abstraction.
//!
//! Every field is a value derived from the execution itself (XDR-decoded
//! results, footprint-diff-derived state changes, ordered event payloads).
//! Diagnostic/debug strings are deliberately **not** part of this model:
//! they are not schema-stable and must never enter a canonical identity.

use soroban_env_host::xdr::{ContractEvent, ContractEventBody, ReadXdr, ScVal};

/// Outcome of a single host-function execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionStatus {
    /// The call returned a value.
    Returned(ScVal),
    /// The call failed with a contract/host error.
    ///
    /// This is a **result**, not a verdict: this core does not decide
    /// whether the failure was expected, unexpected, or a vulnerability.
    /// That is the oracle's job (later phase).
    ContractError {
        /// `ScErrorType` name, e.g. `"Contract"`, `"Budget"`, `"WasmVm"`.
        error_type: String,
        /// Numeric error code (`ScErrorCode`, or contract error code).
        code: u32,
    },
    /// The invocation returned no value.
    Void,
}

/// What happened to one entry in the footprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateChange {
    /// Entry did not exist in the baseline and now exists.
    Created,
    /// Entry existed with a different value.
    Updated,
    /// Entry existed in the baseline and was removed.
    Deleted,
    /// Footprint entry with no observable change.
    Unchanged,
}

/// A baseline-relative state observation for one ledger entry.
///
/// `key_xdr` is the encoded `LedgerKey` and `value_xdr` the encoded
/// `LedgerEntry` when one exists after execution — both are stable byte
/// encodings, safe to compare and to hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateEntry {
    /// Encoded `LedgerKey` XDR.
    pub key_xdr: Vec<u8>,
    /// Encoded `LedgerEntry` XDR after execution; `None` when removed.
    pub value_xdr: Option<Vec<u8>>,
    /// What changed relative to the baseline.
    pub change: StateChange,
}

/// One contract/system event, in emission order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRecord {
    /// Emitting contract id bytes, when the event is contract-scoped.
    pub contract_id: Option<[u8; 32]>,
    /// `ContractEventType` name (`"Contract"`, `"System"`, `"Diagnostic"`).
    pub event_type: String,
    /// Event topics.
    pub topics: Vec<ScVal>,
    /// Event data.
    pub data: ScVal,
}

/// Budget consumption for one execution.
///
/// A fresh budget is used per case, so these counters are per-case, never
/// cumulative across a campaign.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BudgetUsage {
    pub consumed_cpu: u64,
    pub consumed_mem: u64,
    pub remaining_cpu: u64,
    pub remaining_mem: u64,
}

/// SDKT's own view of one contract execution.
///
/// Determinism invariant: for the same WASM bytes, [`crate::FuzzConfig`],
/// and [`crate::FuzzCase`], every field here is byte-identical across
/// processes. There is no timestamp, address, or debug string in this model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Case this observation belongs to, echoed for traceability.
    /// Not part of any canonical hash.
    pub case_id: String,
    /// Function name that was invoked.
    pub function: String,
    /// Call outcome.
    pub status: ExecutionStatus,
    /// State entries from the execution footprint, ordered by key.
    pub state: Vec<StateEntry>,
    /// Emitted events, in emission order (empty when the call failed).
    pub events: Vec<EventRecord>,
    /// Budget consumed/remaining by this execution.
    pub budget: BudgetUsage,
}

impl Observation {
    /// True when the call returned a value or returned nothing — i.e. did
    /// not fail. Says nothing about correctness: an observed success is
    /// not a security conclusion.
    pub fn is_success(&self) -> bool {
        matches!(
            self.status,
            ExecutionStatus::Returned(_) | ExecutionStatus::Void
        )
    }

    /// The returned value, when there was one.
    pub fn return_value(&self) -> Option<&ScVal> {
        match &self.status {
            ExecutionStatus::Returned(v) => Some(v),
            _ => None,
        }
    }

    /// State changes relative to the baseline (excludes `Unchanged`).
    pub fn changes(&self) -> impl Iterator<Item = &StateEntry> {
        self.state
            .iter()
            .filter(|e| e.change != StateChange::Unchanged)
    }
}

/// Map the host's footprint change list into SDKT-owned state entries,
/// diffed against the baseline that was seeded for this case.
///
/// The host emits a change record for every footprint key (including
/// no-ops and read-only entries), so the baseline is what makes
/// `Created` / `Updated` / `Deleted` distinguishable.
///
/// `old_entry_size_bytes_for_rent == 0` is the host's marker for "no old
/// entry existed".
pub(crate) fn state_entries_from_footprint(
    changes: &[soroban_env_host::e2e_invoke::LedgerEntryChange],
    baseline: &std::collections::BTreeMap<Vec<u8>, Vec<u8>>,
) -> Vec<StateEntry> {
    let mut out: Vec<StateEntry> = changes
        .iter()
        .map(|c| {
            let change = match &c.encoded_new_value {
                None => match baseline.get(&c.encoded_key) {
                    Some(_) => StateChange::Deleted,
                    None => StateChange::Unchanged,
                },
                Some(new_value) => match baseline.get(&c.encoded_key) {
                    None => StateChange::Created,
                    Some(old) if old == new_value => StateChange::Unchanged,
                    Some(_) => StateChange::Updated,
                },
            };
            StateEntry {
                key_xdr: c.encoded_key.clone(),
                value_xdr: c.encoded_new_value.clone(),
                change,
            }
        })
        .collect();
    // Stable order independent of footprint iteration order.
    out.sort_by(|a, b| a.key_xdr.cmp(&b.key_xdr));
    out
}

/// Decode emitted contract/system events from their XDR encodings.
pub(crate) fn decode_events(
    encoded: &[Vec<u8>],
) -> Result<Vec<EventRecord>, crate::error::FuzzError> {
    let mut out = Vec::with_capacity(encoded.len());
    for bytes in encoded {
        let event =
            ContractEvent::from_xdr(bytes, soroban_env_host::xdr::Limits::none()).map_err(|e| {
                crate::error::FuzzError::Observation(format!("event did not decode: {e}"))
            })?;
        let (topics, data) = match &event.body {
            ContractEventBody::V0(v0) => (
                v0.topics.iter().cloned().collect::<Vec<ScVal>>(),
                v0.data.clone(),
            ),
        };
        out.push(EventRecord {
            contract_id: event.contract_id.as_ref().map(|c| c.0 .0),
            event_type: format!("{:?}", event.type_),
            topics,
            data,
        });
    }
    Ok(out)
}
