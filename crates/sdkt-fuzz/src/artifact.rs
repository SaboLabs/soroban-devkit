//! Canonical, self-contained finding artifacts (Phase 3).
//!
//! A [`FindingArtifact`] is the *only* serialized form a fuzz finding takes.
//! It is designed so replay needs nothing beyond the artifact, the campaign
//! WASM bytes, and the public Soroban host API:
//!
//! - No debug strings from the host (only schema-stable enum names).
//! - No timestamps, memory addresses, or process state.
//! - No host/cache internals, no `ModuleCache` details.
//! - No network URLs or RPC handles.
//!
//! ## Canonical identity
//!
//! [`FindingArtifact::canonical_bytes`] produces a canonical JSON encoding
//! (recursively sorted object keys, no insignificant whitespace) and
//! [`FindingArtifact::canonical_hash`] derives its SHA-256. The same
//! semantic artifact always produces the same canonical bytes and the same
//! hash, in any process.
//!
//! All payloads (args, state, events, expected values) are carried as
//! **base16 XDR** — the same bytes the host sees — so nothing depends on a
//! Debug/Display format. The WASM itself is not embedded: the artifact
//! carries its SHA-256 and replay verifies the supplied WASM against it
//! (see [`crate::replay`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::auth::AuthMode;
use crate::case::FunctionCall;
use crate::environment::{BudgetPlan, Environment};
use crate::error::FuzzError;
use crate::finding::Finding;
use crate::mutation::Mutation;
use crate::observation::{EventRecord, ExecutionStatus, Observation, StateChange};
use crate::oracle::{Expected, ExpectedBehavior};

/// Artifact schema version. Replay rejects unknown versions instead of
/// guessing.
pub const SCHEMA_VERSION: u32 = 1;

/// Serializable environment snapshot.
///
/// Same shape as [`Environment`], so artifact and runtime model round-trip
/// losslessly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentSnapshot {
    pub sequence_number: u32,
    pub timestamp: u64,
    pub budget: BudgetPlanSnapshot,
}

/// Serializable budget plan. A capped budget is a *synthetic*
/// instruction-cost model, not network-faithful — recorded, not hidden.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BudgetPlanSnapshot {
    Default,
    Capped { cpu: u64, mem: u64 },
}

impl From<&Environment> for EnvironmentSnapshot {
    fn from(e: &Environment) -> Self {
        Self {
            sequence_number: e.ledger.sequence_number,
            timestamp: e.ledger.timestamp,
            budget: match &e.budget {
                BudgetPlan::Default => BudgetPlanSnapshot::Default,
                BudgetPlan::Capped { cpu, mem } => BudgetPlanSnapshot::Capped {
                    cpu: *cpu,
                    mem: *mem,
                },
            },
        }
    }
}

impl Default for EnvironmentSnapshot {
    fn default() -> Self {
        Self::from(&Environment::default())
    }
}

impl From<&EnvironmentSnapshot> for Environment {
    fn from(s: &EnvironmentSnapshot) -> Self {
        Environment {
            ledger: crate::config::LedgerConfig {
                sequence_number: s.sequence_number,
                timestamp: s.timestamp,
            },
            budget: match &s.budget {
                BudgetPlanSnapshot::Default => BudgetPlan::Default,
                BudgetPlanSnapshot::Capped { cpu, mem } => BudgetPlan::Capped {
                    cpu: *cpu,
                    mem: *mem,
                },
            },
        }
    }
}

/// Serializable campaign configuration snapshot: exactly what replay needs
/// to rebuild the campaign context. No coverage/corpus/parallel/network
/// fields exist — by design.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampaignConfig {
    /// SHA-256 of the campaign WASM (lowercase hex).
    pub wasm_sha256: String,
    /// Base PRNG seed, lowercase hex.
    pub seed_hex: String,
    /// Cases requested.
    pub cases: usize,
    /// Function filter, when the campaign was scoped to one function.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(default)]
    pub sequence_length: usize,
    /// Every Nth case is a sequence (0 = never).
    #[serde(default)]
    pub sequence_every: usize,
    #[serde(default)]
    pub mutations_per_case: usize,
    #[serde(default)]
    pub generation: GenerationCapsSnapshot,
    #[serde(default)]
    pub environment: EnvironmentSnapshot,
    /// Auth mode names this campaign executed under.
    #[serde(default)]
    pub auth_modes: Vec<String>,
}

/// Generation caps as data (mirrors [`crate::generator::GenerationCaps`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationCapsSnapshot {
    pub max_depth: u32,
    pub max_vec_len: usize,
    pub max_byte_len: usize,
    pub max_map_entries: usize,
}

impl Default for GenerationCapsSnapshot {
    fn default() -> Self {
        let d = crate::generator::GenerationCaps::default();
        Self {
            max_depth: d.max_depth,
            max_vec_len: d.max_vec_len,
            max_byte_len: d.max_byte_len,
            max_map_entries: d.max_map_entries,
        }
    }
}

impl From<crate::generator::GenerationCaps> for GenerationCapsSnapshot {
    fn from(c: crate::generator::GenerationCaps) -> Self {
        Self {
            max_depth: c.max_depth,
            max_vec_len: c.max_vec_len,
            max_byte_len: c.max_byte_len,
            max_map_entries: c.max_map_entries,
        }
    }
}

impl From<GenerationCapsSnapshot> for crate::generator::GenerationCaps {
    fn from(c: GenerationCapsSnapshot) -> Self {
        crate::generator::GenerationCaps {
            max_depth: c.max_depth,
            max_vec_len: c.max_vec_len,
            max_byte_len: c.max_byte_len,
            max_map_entries: c.max_map_entries,
        }
    }
}

/// Serializable mutation record; `mutation_id` is the identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationSnapshot {
    pub mutation_id: String,
    pub arg_index: usize,
    /// Operator name from [`crate::mutation::Operator::name`].
    pub operator: String,
    /// The mutated argument value, base16 XDR.
    pub value_xdr: String,
    /// Full argument vector after mutation, base16 XDR each.
    pub args_xdr: Vec<String>,
}

impl MutationSnapshot {
    fn from_mutation(m: &Mutation) -> Self {
        Self {
            mutation_id: m.mutation_id.clone(),
            arg_index: m.arg_index,
            operator: m.operator.name().to_string(),
            value_xdr: xdr_hex(&m.value),
            args_xdr: m.args.iter().map(xdr_hex).collect(),
        }
    }
}

/// Serializable observation — everything replay verifies against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationSnapshot {
    pub case_id: String,
    pub function: String,
    pub status: StatusSnapshot,
    pub state: Vec<StateEntrySnapshot>,
    pub events: Vec<EventSnapshot>,
    pub budget: BudgetSnapshot,
}

/// Stable status representation: no Debug formats.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StatusSnapshot {
    Returned { value_xdr: String },
    Void,
    ContractError { error_type: String, code: u32 },
}

impl ObservationSnapshot {
    pub(crate) fn from_observation(o: &Observation) -> Self {
        Self {
            case_id: o.case_id.clone(),
            function: o.function.clone(),
            status: match &o.status {
                ExecutionStatus::Returned(v) => StatusSnapshot::Returned {
                    value_xdr: xdr_hex(v),
                },
                ExecutionStatus::Void => StatusSnapshot::Void,
                ExecutionStatus::ContractError { error_type, code } => {
                    StatusSnapshot::ContractError {
                        error_type: error_type.clone(),
                        code: *code,
                    }
                }
            },
            state: o
                .state
                .iter()
                .map(|s| StateEntrySnapshot {
                    key_xdr: hex(&s.key_xdr),
                    value_xdr: s.value_xdr.as_ref().map(|v| hex(v)),
                    change: match s.change {
                        StateChange::Created => "created",
                        StateChange::Updated => "updated",
                        StateChange::Deleted => "deleted",
                        StateChange::Unchanged => "unchanged",
                    }
                    .to_string(),
                })
                .collect(),
            events: o.events.iter().map(snapshot_event).collect(),
            budget: BudgetSnapshot {
                consumed_cpu: o.budget.consumed_cpu,
                consumed_mem: o.budget.consumed_mem,
                remaining_cpu: o.budget.remaining_cpu,
                remaining_mem: o.budget.remaining_mem,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateEntrySnapshot {
    pub key_xdr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_xdr: Option<String>,
    pub change: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    pub event_type: String,
    pub topics_xdr: Vec<String>,
    pub data_xdr: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    pub consumed_cpu: u64,
    pub consumed_mem: u64,
    pub remaining_cpu: u64,
    pub remaining_mem: u64,
}

/// Serializable oracle expectation — the declared rule a finding violated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedSnapshot {
    #[serde(default)]
    pub behavior: BehaviorSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Vec<StateExpectation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<EventSnapshot>>,
    #[serde(default)]
    pub auth_required: bool,
    #[serde(default)]
    pub resource_limit_is_finding: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BehaviorSnapshot {
    Success {
        expect_return: Option<String>,
    },
    Error {
        error_type: String,
        code: u32,
    },
    #[default]
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateExpectation {
    pub key_xdr: String,
    pub value_xdr: String,
}

impl ExpectedSnapshot {
    fn from_expected(e: &Expected) -> Self {
        Self {
            behavior: match &e.behavior {
                ExpectedBehavior::Success { expect_return } => BehaviorSnapshot::Success {
                    expect_return: expect_return.as_ref().map(xdr_hex),
                },
                ExpectedBehavior::Error { error_type, code } => BehaviorSnapshot::Error {
                    error_type: error_type.clone(),
                    code: *code,
                },
                ExpectedBehavior::Any => BehaviorSnapshot::Any,
            },
            state: e.state.as_ref().map(|m| {
                m.iter()
                    .map(|(k, v)| StateExpectation {
                        key_xdr: hex(k),
                        value_xdr: hex(v),
                    })
                    .collect()
            }),
            events: e
                .events
                .as_ref()
                .map(|ev| ev.iter().map(snapshot_event).collect()),
            auth_required: e.auth_required,
            resource_limit_is_finding: e.resource_limit_is_finding,
        }
    }

    /// Reconstruct the runtime expectation for replay.
    pub fn to_expected(&self) -> Expected {
        Expected {
            behavior: match &self.behavior {
                BehaviorSnapshot::Success { expect_return } => ExpectedBehavior::Success {
                    expect_return: expect_return.as_deref().and_then(unhex_xdr),
                },
                BehaviorSnapshot::Error { error_type, code } => ExpectedBehavior::Error {
                    error_type: error_type.clone(),
                    code: *code,
                },
                BehaviorSnapshot::Any => ExpectedBehavior::Any,
            },
            state: self.state.as_ref().map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| Some((unhex(&e.key_xdr)?, unhex(&e.value_xdr)?)))
                    .collect::<BTreeMap<Vec<u8>, Vec<u8>>>()
            }),
            events: self
                .events
                .as_ref()
                .map(|ev| ev.iter().map(restore_event).collect()),
            auth_required: self.auth_required,
            resource_limit_is_finding: self.resource_limit_is_finding,
        }
    }
}

fn snapshot_event(e: &EventRecord) -> EventSnapshot {
    EventSnapshot {
        contract_id: e.contract_id.map(|c| hex(&c)),
        event_type: e.event_type.clone(),
        topics_xdr: e.topics.iter().map(xdr_hex).collect(),
        data_xdr: xdr_hex(&e.data),
    }
}

fn restore_event(e: &EventSnapshot) -> EventRecord {
    EventRecord {
        contract_id: e.contract_id.as_deref().and_then(unhex_32),
        event_type: e.event_type.clone(),
        topics: e.topics_xdr.iter().filter_map(|t| unhex_xdr(t)).collect(),
        data: unhex_xdr(&e.data_xdr).unwrap_or(soroban_env_host::xdr::ScVal::Void),
    }
}

/// Serializable minimization record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinimizationSnapshot {
    pub attempted: bool,
    /// Always the strategy phrase; never a global-minimum claim.
    pub strategy: String,
    pub original_complexity: usize,
    pub minimized_complexity: usize,
    pub preserved: bool,
    pub reductions: Vec<ReductionSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReductionSnapshot {
    pub kind: String,
    pub detail: String,
    pub complexity_after: usize,
}

/// One sequence step: function + base16-XDR arguments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceStepSnapshot {
    pub function: String,
    pub args_xdr: Vec<String>,
}

/// The artifact itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingArtifact {
    pub schema_version: u32,
    pub campaign: CampaignConfig,
    pub case_id: String,
    pub function: String,
    pub sequence: Vec<SequenceStepSnapshot>,
    pub auth_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation: Option<MutationSnapshot>,
    pub expected: ExpectedSnapshot,
    pub observation: ObservationSnapshot,
    pub reason_code: String,
    pub minimization: MinimizationSnapshot,
}

impl FindingArtifact {
    /// Build an artifact from a classified, minimized finding.
    pub fn from_finding(
        finding: &Finding,
        campaign: CampaignConfig,
        steps: &[FunctionCall],
        wasm: &[u8],
    ) -> Self {
        let auth_mode = AuthMode::from_name(&finding.identity.auth_mode)
            .unwrap_or(AuthMode::NoAuth)
            .name()
            .to_string();
        let mut campaign = campaign;
        campaign.wasm_sha256 = hex(&Sha256::digest(wasm));
        Self {
            schema_version: SCHEMA_VERSION,
            campaign,
            case_id: finding.identity.case_id.clone(),
            function: finding.identity.function.clone(),
            sequence: steps
                .iter()
                .map(|s| SequenceStepSnapshot {
                    function: s.function.clone(),
                    args_xdr: s.args.iter().map(xdr_hex).collect(),
                })
                .collect(),
            auth_mode,
            mutation: finding
                .mutation
                .as_ref()
                .map(MutationSnapshot::from_mutation),
            expected: ExpectedSnapshot::from_expected(&finding.expected),
            observation: ObservationSnapshot::from_observation(&finding.observation),
            reason_code: finding.reason_code.name().to_string(),
            minimization: MinimizationSnapshot {
                attempted: finding.minimization.attempted,
                strategy: finding.minimization.strategy.to_string(),
                original_complexity: finding.minimization.original_complexity,
                minimized_complexity: finding.minimization.minimized_complexity,
                preserved: finding.minimization.preserved,
                reductions: finding
                    .minimization
                    .reductions
                    .iter()
                    .map(|r| ReductionSnapshot {
                        kind: r.kind.clone(),
                        detail: r.detail.clone(),
                        complexity_after: r.complexity_after,
                    })
                    .collect(),
            },
        }
    }

    /// SHA-256 of the canonical JSON encoding: the artifact's identity.
    pub fn canonical_hash(&self) -> String {
        hex(&Sha256::digest(self.canonical_bytes()))
    }

    /// Canonical serialization: recursively sorted object keys, no
    /// insignificant whitespace.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("artifact is serializable by construction");
        let canonical = canonicalize(value);
        serde_json::to_vec(&canonical).expect("canonical JSON is serializable")
    }

    /// Canonical JSON as a string (what the CLI writes to disk).
    pub fn canonical_json(&self) -> String {
        String::from_utf8_lossy(&self.canonical_bytes()).into_owned()
    }

    /// The declared expectation that was violated.
    pub fn expected(&self) -> Expected {
        self.expected.to_expected()
    }

    /// Steps to replay, decoded from base16 XDR.
    pub fn steps(&self) -> Result<Vec<FunctionCall>, FuzzError> {
        self.sequence
            .iter()
            .map(|s| {
                let args = s
                    .args_xdr
                    .iter()
                    .map(|a| {
                        unhex_xdr(a).ok_or_else(|| {
                            FuzzError::InvalidArtifact(
                                "argument did not decode as ScVal XDR".to_string(),
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, FuzzError>>()?;
                Ok(FunctionCall::new(s.function.clone(), args))
            })
            .collect()
    }

    /// The runtime environment to replay under.
    pub fn environment(&self) -> Environment {
        Environment::from(&self.campaign.environment)
    }

    /// Deterministic seed, decoded from the campaign snapshot.
    pub fn seed(&self) -> Result<[u8; 32], FuzzError> {
        let bytes = unhex(&self.campaign.seed_hex)
            .ok_or_else(|| FuzzError::InvalidArtifact("campaign seed is not hex".to_string()))?;
        bytes
            .try_into()
            .map_err(|_| FuzzError::InvalidArtifact("campaign seed is not 32 bytes".to_string()))
    }
}

/// Recursively sort JSON object keys so key order never affects the
/// canonical bytes. Arrays keep their (deterministic) order.
fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (k, v) in entries {
                out.insert(k, canonicalize(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize).collect()),
        other => other,
    }
}

// --- base16 + XDR helpers --------------------------------------------------

/// Encode a value's XDR as lowercase hex.
pub(crate) fn xdr_hex<T: soroban_env_host::xdr::WriteXdr>(v: &T) -> String {
    use soroban_env_host::xdr::Limits;
    hex(&v.to_xdr(Limits::none()).unwrap_or_default())
}

/// Decode a `ScVal` from lowercase-hex XDR.
pub(crate) fn unhex_xdr(hex_str: &str) -> Option<soroban_env_host::xdr::ScVal> {
    use soroban_env_host::xdr::{Limits, ReadXdr};
    let bytes = unhex(hex_str)?;
    soroban_env_host::xdr::ScVal::from_xdr(bytes.as_slice(), Limits::none()).ok()
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let (pairs, remainder) = s.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let hi = hex_digit(pair[0])?;
        let lo = hex_digit(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn unhex_32(s: &str) -> Option<[u8; 32]> {
    unhex(s)?.try_into().ok()
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Public helper: encode one `ScVal` as base16 XDR (tests + CLI use this
/// for building expectations in tests; the canonical format itself always
/// uses the internal `xdr_hex`).
pub fn encode_scval_xdr_hex(v: &soroban_env_host::xdr::ScVal) -> String {
    xdr_hex(v)
}

/// Lowercase-hex encode.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}
