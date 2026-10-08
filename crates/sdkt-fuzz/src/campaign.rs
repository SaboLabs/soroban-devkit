//! Campaign orchestration: generate → mutate → execute → classify →
//! minimize → canonical artifact.
//!
//! Pipeline (all in-memory unless artifacts are requested):
//!
//! ```text
//! WASM → ContractSpec → CampaignInput → generate/mutate → sequence/env/auth
//!      → baseline → fresh Host + shared ModuleCache → execute
//!      → Observation → Oracle → FINDING → Minimizer → FindingArtifact
//! ```
//!
//! ## Auth wiring (Phase 3)
//!
//! [`CampaignInput::auth_mode`] selects the authorization mode for every
//! case. Entries are built per step by [`crate::auth`] and installed through
//! the host's enforcing authorization path. Two empirically verified host
//! behaviors are relied on, and nothing more:
//!
//! 1. a `SourceAccount` entry whose root invocation matches the executed host
//!    function authenticates the transaction source;
//! 2. a non-matching root, or an `AddressV2` entry carrying an invalid
//!    signature, fails with an `Auth` error.
//!
//! The host does **not** report unconsumed authorization entries, and its
//! public API exposes no authorization trace, so "an entry existed but was
//! never used" is not observable. That limitation is recorded, not papered
//! over: see the Phase 3 verification report.

use sdkt_wasm::{parse_contract_spec, ContractSpec};
use sha2::Digest;

use crate::artifact::{CampaignConfig as ArtifactCampaignConfig, FindingArtifact};
use crate::auth::AuthMode;
use crate::case::FunctionCall;
use crate::environment::Environment;
use crate::error::FuzzError;
use crate::executor::Executor;
use crate::finding::{CaseIdentity, Finding, SkippedFunction};
use crate::generator::{generate_call, GenerationCaps};
use crate::minimizer::{minimize_with_oracle, MinimizationTarget};
use crate::mutation::{mutate_arg, plan_mutation, Mutation, MutationError};
use crate::oracle::{Classification, Expected, Oracle};
use crate::sequence::execute_sequence_auth;

/// What a campaign case does with the generated/mutated input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaseKind {
    /// One call.
    Single,
    /// Ordered call sequence; the final observation is classified.
    Sequence(usize),
}

/// A generated campaign case, pre-execution.
#[derive(Clone, Debug)]
pub struct CampaignCase {
    pub case_id: String,
    pub kind: CaseKind,
    pub function: String,
    /// Steps: one for `Single`, N for `Sequence`.
    pub steps: Vec<FunctionCall>,
    pub auth_mode: AuthMode,
    /// Mutation that produced this case, if any.
    pub mutation: Option<Mutation>,
}

/// Everything a campaign needs, all deterministic.
pub struct CampaignInput {
    pub wasm: Vec<u8>,
    pub spec: ContractSpec,
    pub seed: [u8; 32],
    pub cases: usize,
    /// Restrict the campaign to one function. An unknown or unsupported
    /// function is reported as a skip, never silently ignored.
    pub function: Option<String>,
    /// Sequence length when generating `CaseKind::Sequence` cases.
    pub sequence_length: usize,
    /// Every `n`-th case is a sequence (0 = never).
    pub sequence_every: usize,
    pub generation: GenerationCaps,
    /// Mutations per generated case (0 = generation only).
    pub mutations_per_case: usize,
    /// Deterministic environment for all executions.
    pub environment: Environment,
    /// Authorization mode every case executes under.
    pub auth_mode: AuthMode,
}

impl CampaignInput {
    pub fn new(wasm: &[u8], seed: [u8; 32]) -> Result<Self, FuzzError> {
        let spec = parse_contract_spec(wasm)
            .map_err(|e| FuzzError::InvalidConfig(format!("ContractSpec: {e}")))?;
        Ok(Self {
            wasm: wasm.to_vec(),
            spec,
            seed,
            cases: 16,
            function: None,
            sequence_length: 2,
            sequence_every: 0,
            generation: GenerationCaps::default(),
            mutations_per_case: 2,
            environment: Environment::default(),
            auth_mode: AuthMode::NoAuth,
        })
    }

    /// The function names this campaign may exercise, honouring the
    /// `function` filter.
    pub fn selectable(&self) -> (Vec<String>, Vec<SkippedFunction>) {
        let (all_ok, all_skipped) = crate::finding::selectable(&self.spec);
        match &self.function {
            None => (all_ok, all_skipped),
            Some(want) => {
                if all_ok.iter().any(|f| f == want) {
                    (vec![want.clone()], Vec::new())
                } else if let Some(skip) = all_skipped.iter().find(|s| &s.function == want) {
                    (Vec::new(), vec![skip.clone()])
                } else {
                    (
                        Vec::new(),
                        vec![SkippedFunction {
                            function: want.clone(),
                            reason: "function not present in ContractSpec".to_string(),
                        }],
                    )
                }
            }
        }
    }

    /// Artifact-ready configuration snapshot for this campaign.
    pub fn artifact_config(&self) -> ArtifactCampaignConfig {
        ArtifactCampaignConfig {
            wasm_sha256: hex(&sha2::Sha256::digest(&self.wasm)),
            seed_hex: hex(&self.seed),
            cases: self.cases,
            function: self.function.clone(),
            sequence_length: self.sequence_length,
            sequence_every: self.sequence_every,
            mutations_per_case: self.mutations_per_case,
            generation: self.generation.into(),
            environment: crate::artifact::EnvironmentSnapshot::from(&self.environment),
            auth_modes: vec![self.auth_mode.name().to_string()],
        }
    }
}

/// In-memory campaign result.
#[derive(Clone, Debug, Default)]
pub struct CampaignResult {
    /// Case ids whose observations classified as PASS.
    pub passed: Vec<String>,
    /// Case ids whose observations matched a declared EXPECTED_ERROR.
    pub expected_errors: Vec<String>,
    /// Classified findings (with minimization attached).
    pub findings: Vec<Finding>,
    /// Functions skipped, with reasons (never silent).
    pub skipped_functions: Vec<SkippedFunction>,
    /// Cases actually executed.
    pub executed: usize,
}

impl CampaignResult {
    /// Minimized findings only.
    pub fn minimized_findings(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.minimization.attempted)
    }
}

/// Run the campaign: generate → mutate → execute → classify → minimize.
pub fn run_campaign(
    input: &CampaignInput,
    expectations: &std::collections::BTreeMap<String, Expected>,
) -> Result<CampaignResult, FuzzError> {
    run_campaign_inner(input, expectations).map(|(result, _)| result)
}

/// Run the campaign and produce one canonical artifact per finding.
pub fn run_campaign_artifacts(
    input: &CampaignInput,
    expectations: &std::collections::BTreeMap<String, Expected>,
) -> Result<(CampaignResult, Vec<FindingArtifact>), FuzzError> {
    run_campaign_inner(input, expectations)
}

fn run_campaign_inner(
    input: &CampaignInput,
    expectations: &std::collections::BTreeMap<String, Expected>,
) -> Result<(CampaignResult, Vec<FindingArtifact>), FuzzError> {
    let executor = Executor::new(&input.wasm, default_config(input)?)?;
    let artifact_config = input.artifact_config();
    let (functions, skipped) = input.selectable();
    if functions.is_empty() {
        return Ok((
            CampaignResult {
                skipped_functions: skipped,
                ..Default::default()
            },
            Vec::new(),
        ));
    }

    let mut result = CampaignResult {
        skipped_functions: skipped,
        ..Default::default()
    };
    let mut artifacts: Vec<FindingArtifact> = Vec::new();

    for case_number in 0..input.cases {
        let function = functions[case_number % functions.len()].clone();
        let case_id = format!("case{case_number:04}");
        let is_sequence = input.sequence_every > 0
            && input.sequence_length > 1
            && case_number % input.sequence_every == 0;

        // --- Generate the base call (or sequence) ----------------------
        let base_call = generate_call(
            &input.spec,
            &function,
            &input.seed,
            &case_id,
            input.generation,
        )
        .map_err(|e| FuzzError::InvalidConfig(format!("generator: {e}")))?;
        let mut steps: Vec<FunctionCall> = if is_sequence {
            (0..input.sequence_length)
                .map(|i| {
                    let seq_case_id = format!("{case_id}/gen{i}");
                    let generated = generate_call(
                        &input.spec,
                        &function,
                        &input.seed,
                        &seq_case_id,
                        input.generation,
                    )
                    .map_err(|e| FuzzError::InvalidConfig(format!("generator: {e}")))?;
                    Ok(FunctionCall::new(function.clone(), generated.args))
                })
                .collect::<Result<Vec<_>, FuzzError>>()?
        } else {
            vec![FunctionCall::new(function.clone(), base_call.args)]
        };

        // --- Mutations (typed, deterministic, non-targets preserved) ---
        let mut mutation_record: Option<Mutation> = None;
        if input.mutations_per_case > 0 && !is_sequence {
            let param_types: Vec<&sdkt_wasm::ContractType> = input
                .spec
                .functions
                .iter()
                .find(|f| f.name == function)
                .map(|f| f.parameters.iter().map(|p| &p.type_).collect())
                .unwrap_or_default();
            let args = &mut steps[0].args;
            for m in 0..input.mutations_per_case {
                let Some((arg_index, operator)) =
                    plan_mutation(&param_types, &input.seed, &case_id, m)
                else {
                    break;
                };
                if arg_index >= args.len() {
                    continue;
                }
                let t = param_types[arg_index];
                match mutate_arg(
                    t,
                    args,
                    arg_index,
                    operator,
                    &input.seed,
                    &case_id,
                    input.generation,
                ) {
                    Ok(mutation) => {
                        *args = mutation.args.clone();
                        mutation_record = Some(mutation);
                    }
                    Err(MutationError::NoApplicableMutation)
                    | Err(MutationError::ArgIndexOutOfRange { .. }) => continue,
                    Err(e) => return Err(FuzzError::Execution(format!("mutation: {e}"))),
                }
            }
        }

        // --- Expectation & oracle --------------------------------------
        let expected = expectations.get(&function).cloned().unwrap_or_default();
        let oracle = Oracle::new(expected.clone());

        // --- Authorization entries for this case -----------------------
        let auth_per_step = auth_for_steps(&executor, &steps, input.auth_mode, &case_id)?;

        // --- Execute ----------------------------------------------------
        let observation = if is_sequence {
            execute_sequence_auth(
                &executor,
                &[],
                &input.environment,
                &steps,
                &case_id,
                &auth_per_step,
            )?
            .final_observation
        } else {
            let case = executor.case(&case_id, steps[0].clone(), Vec::new());
            match auth_per_step.first() {
                Some(entries) => executor.execute_with_auth(&case, &input.environment, entries)?,
                None => executor.execute_with(&case, &input.environment)?,
            }
        };

        // --- Classify ---------------------------------------------------
        let classification = oracle.classify(&observation);
        result.executed += 1;
        match classification {
            Classification::Pass => result.passed.push(case_id),
            Classification::ExpectedError => result.expected_errors.push(case_id),
            Classification::Finding(reason) => {
                let outcome = minimize_with_oracle(
                    &executor,
                    MinimizationTarget {
                        steps: steps.clone(),
                        baseline: Vec::new(),
                        environment: input.environment.clone(),
                    },
                    Some(&oracle),
                    &Classification::Finding(reason),
                    observation.is_success(),
                )?;
                steps = outcome.steps.clone();

                // Re-observe the minimized case for the finding record.
                let minimized_auth = auth_for_steps(&executor, &steps, input.auth_mode, &case_id)?;
                let minimized_observation = if steps.len() > 1 {
                    execute_sequence_auth(
                        &executor,
                        &[],
                        &input.environment,
                        &steps,
                        &case_id,
                        &minimized_auth,
                    )?
                    .final_observation
                } else {
                    let case = executor.case(&case_id, steps[0].clone(), Vec::new());
                    match minimized_auth.first() {
                        Some(entries) => {
                            executor.execute_with_auth(&case, &input.environment, entries)?
                        }
                        None => executor.execute_with(&case, &input.environment)?,
                    }
                };

                let identity = CaseIdentity {
                    case_id: case_id.clone(),
                    wasm_hash: hex(&executor.wasm_hash()),
                    function: function.clone(),
                    mutation_id: mutation_record.as_ref().map(|m| m.mutation_id.clone()),
                    operator: mutation_record
                        .as_ref()
                        .map(|m| m.operator.name().to_string()),
                    auth_mode: input.auth_mode.name().to_string(),
                };
                let mut minimization = outcome.minimization;
                minimization.preserved =
                    oracle.classify(&minimized_observation) == Classification::Finding(reason);

                let finding = Finding {
                    reason_code: reason,
                    identity,
                    case: executor.case(&case_id, steps[0].clone(), Vec::new()),
                    mutation: mutation_record,
                    environment: input.environment.clone(),
                    expected,
                    observation: minimized_observation,
                    minimization,
                };
                artifacts.push(FindingArtifact::from_finding(
                    &finding,
                    artifact_config.clone(),
                    &steps,
                    &input.wasm,
                ));
                result.findings.push(finding);
            }
        }
    }

    Ok((result, artifacts))
}

/// Build the per-step authorization entries for a case.
///
/// `NoAuth` yields an empty vector (the host enforces with no
/// authorizations). Other modes build one entry per step via
/// [`crate::auth::invoke_auth_entries`], with that step's call as the root.
fn auth_for_steps(
    executor: &Executor,
    steps: &[FunctionCall],
    mode: AuthMode,
    case_id: &str,
) -> Result<Vec<Vec<Vec<u8>>>, FuzzError> {
    if mode == AuthMode::NoAuth {
        return Ok(Vec::new());
    }
    let mut per_step = Vec::with_capacity(steps.len());
    for (i, call) in steps.iter().enumerate() {
        let contract = executor
            .case(format!("{case_id}/step{i}"), call.clone(), Vec::new())
            .contract_address();
        per_step.push(invoke_auth_entries_for(executor, i, mode, &contract, call)?);
    }
    Ok(per_step)
}

fn invoke_auth_entries_for(
    _executor: &Executor,
    _step: usize,
    mode: AuthMode,
    contract: &soroban_env_host::xdr::ScAddress,
    call: &FunctionCall,
) -> Result<Vec<Vec<u8>>, FuzzError> {
    crate::auth::invoke_auth_entries(mode, contract, call)
}

fn default_config(input: &CampaignInput) -> Result<crate::config::FuzzConfig, FuzzError> {
    let cfg = crate::config::FuzzConfig {
        seed: input.seed,
        ..Default::default()
    };
    input.environment.validate(&cfg)?;
    Ok(cfg)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Convenience: a one-line summary (logging only).
pub fn summarize(result: &CampaignResult) -> String {
    format!(
        "executed={} passed={} expected_errors={} findings={} skipped={}",
        result.executed,
        result.passed.len(),
        result.expected_errors.len(),
        result.findings.len(),
        result.skipped_functions.len(),
    )
}
