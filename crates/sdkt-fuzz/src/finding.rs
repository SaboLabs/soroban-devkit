//! Structured findings.
//!
//! A [`Finding`] carries everything Phase 3 needs to write a
//! self-contained artifact — but this module deliberately does **not**
//! serialize anything, and does not compute a canonical artifact hash.
//! That is Phase 3's job.

use sdkt_wasm::ContractSpec;

use crate::auth::AuthMode;
use crate::case::FuzzCase;
use crate::environment::Environment;
use crate::minimizer::Minimization;
use crate::mutation::Mutation;
use crate::observation::Observation;
use crate::oracle::{Classification, Expected, ReasonCode};

/// Deterministic identity of the case a finding came from. No timestamps,
/// no addresses, no debug strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseIdentity {
    /// Deterministic case id (`case_id` label, not an allocation).
    pub case_id: String,
    /// Compiled WASM SHA-256 (hex).
    pub wasm_hash: String,
    /// Function invoked.
    pub function: String,
    /// Mutation identity, when the case came from a mutation.
    pub mutation_id: Option<String>,
    /// Mutation operator name, when applicable.
    pub operator: Option<String>,
    /// Auth mode name.
    pub auth_mode: String,
}

/// One finding: enough structure for Phase 3, none of its serialization.
#[derive(Clone, Debug)]
pub struct Finding {
    /// Why the oracle raised it.
    pub reason_code: ReasonCode,
    /// Deterministic case identity.
    pub identity: CaseIdentity,
    /// The case exactly as executed (post-minimization when minimized).
    pub case: FuzzCase,
    /// The mutation that produced the case, if any.
    pub mutation: Option<Mutation>,
    /// Environment the case ran under.
    pub environment: Environment,
    /// Declared expectation that was violated.
    pub expected: Expected,
    /// What actually happened.
    pub observation: Observation,
    /// Minimization result (identity when nothing was reduced).
    pub minimization: Minimization,
}

impl Finding {
    /// Build an (unminimized) finding from a classified observation.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reason_code: ReasonCode,
        case: FuzzCase,
        mutation: Option<Mutation>,
        environment: Environment,
        expected: Expected,
        observation: Observation,
        wasm_hash: [u8; 32],
        auth_mode: AuthMode,
    ) -> Self {
        let identity = CaseIdentity {
            case_id: case.case_id.clone(),
            wasm_hash: hex(&wasm_hash),
            function: case.call.function.clone(),
            mutation_id: mutation.as_ref().map(|m| m.mutation_id.clone()),
            operator: mutation.as_ref().map(|m| m.operator.name().to_string()),
            auth_mode: auth_mode.name().to_string(),
        };
        let minimization = Minimization::not_attempted(&case);
        Self {
            reason_code,
            identity,
            case,
            mutation,
            environment,
            expected,
            observation,
            minimization,
        }
    }

    /// True when `classification` names this finding's reason.
    pub fn matches(&self, classification: &Classification) -> bool {
        matches!(classification, Classification::Finding(r) if *r == self.reason_code)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A campaign's function-level skip record: explicit, never silent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedFunction {
    pub function: String,
    pub reason: String,
}

/// Parse a spec for a campaign, keeping the skip records.
pub fn selectable(spec: &ContractSpec) -> (Vec<String>, Vec<SkippedFunction>) {
    let (ok, skipped) = crate::generator::selectable_functions(spec);
    (
        ok,
        skipped
            .into_iter()
            .map(|(function, reason)| SkippedFunction { function, reason })
            .collect(),
    )
}
