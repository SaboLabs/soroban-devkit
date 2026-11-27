//! Structured findings.
//!
//! A [`Finding`] carries everything Phase 3 needs to write a
//! self-contained artifact — but this module deliberately does **not**
//! serialize anything, and does not compute a canonical artifact hash.
//! That is Phase 3's job.

use sdkt_wasm::ContractSpec;

use crate::auth::AuthMode;
use crate::case::{FunctionCall, FuzzCase};
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
    /// Every mutation identity applied to the case, in application order.
    pub mutation_ids: Vec<String>,
    /// Every mutation operator name applied to the case, in application order.
    pub operators: Vec<String>,
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
    /// The case exactly as executed (post-minimization when minimized):
    /// the **first** step of [`Finding::steps`] — never a different step
    /// of the sequence.
    pub case: FuzzCase,
    /// The full executed step sequence, in order — the executed input the
    /// `observation` belongs to (`observation` is the **final** step's
    /// observation of this sequence; for a single step it is that step's
    /// only observation). Single-step findings hold exactly one step,
    /// equal to `case.call`. This mirrors the artifact's `sequence`, so a
    /// finding and its artifact always describe the same execution.
    pub steps: Vec<FunctionCall>,
    /// The mutation that produced the case, if any.
    pub mutation: Option<Mutation>,
    /// Full mutation chain, in application order (last entry equals `mutation`).
    pub mutations: Vec<Mutation>,
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
    ///
    /// `case` is the first step's case (`case.call == steps[0]`) and
    /// `steps` is the full executed sequence; `observation` must be the
    /// final step's observation of that sequence. The unminimized
    /// complexity record covers **all** of `steps`, not just `case.call`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reason_code: ReasonCode,
        case: FuzzCase,
        steps: Vec<FunctionCall>,
        mutation: Option<Mutation>,
        environment: Environment,
        expected: Expected,
        observation: Observation,
        wasm_hash: [u8; 32],
        auth_mode: AuthMode,
    ) -> Self {
        assert!(
            !steps.is_empty() && steps[0] == case.call,
            "finding steps must be the executed sequence and start with case.call"
        );
        let identity = CaseIdentity {
            case_id: case.case_id.clone(),
            wasm_hash: hex(&wasm_hash),
            function: case.call.function.clone(),
            mutation_id: mutation.as_ref().map(|m| m.mutation_id.clone()),
            operator: mutation.as_ref().map(|m| m.operator.name().to_string()),
            mutation_ids: mutation.iter().map(|m| m.mutation_id.clone()).collect(),
            operators: mutation
                .iter()
                .map(|m| m.operator.name().to_string())
                .collect(),
            auth_mode: auth_mode.name().to_string(),
        };
        let minimization = Minimization::not_attempted(&steps);
        Self {
            reason_code,
            identity,
            case,
            steps,
            mutations: mutation.iter().cloned().collect(),
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
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
