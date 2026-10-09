//! `sdkt-fuzz` — Soroban-aware deterministic fuzzing and execution core for
//! SDKT release assurance.
//!
//! This crate owns the Soroban host boundary for SDKT. It compiles contract
//! WASM once into a shared `ModuleCache`, executes each fuzz case against a
//! **fresh** `soroban_env_host::Host`, and reports results as an SDKT-owned
//! [`Observation`] that no host type leaks through.
//!
//! ## Scope
//!
//! Phase 1 (execution core):
//!
//! - execution configuration ([`FuzzConfig`], [`BudgetConfig`])
//! - fuzz case input model ([`FuzzCase`], [`FunctionCall`])
//! - fresh-Host execution ([`Executor`])
//! - typed observation ([`Observation`], [`ExecutionStatus`], [`StateEntry`])
//!
//! Phase 2 (generation → oracle → minimization):
//!
//! - deterministic typed generation from `sdkt-wasm`'s `ContractSpec`
//!   ([`generator`])
//! - type-aware semantic mutation ([`mutation`])
//! - deterministic stateful sequences ([`sequence`])
//! - deterministic environment model ([`environment`])
//! - explicit authorization model ([`auth`])
//! - explicit oracle: PASS / EXPECTED_ERROR / FINDING ([`oracle`])
//! - structured findings ([`Finding`])
//! - deterministic minimization ([`minimizer`])
//! - in-memory campaign orchestration ([`run_campaign`])
//!
//! Out of scope (Phase 3+): artifact serialization, replay, CLI,
//! release-assurance integration. Also deliberately absent: coverage, corpus,
//! byte mutation, parallel execution, LLM/AI, and any RPC/network path.
//!
//! ## Security model (what this crate does and does not claim)
//!
//! - **Explicit oracle only.** A finding exists only where a rule was
//!   explicitly declared (expected behavior, expected state/events, or a
//!   declared auth requirement). Undeclared conditions can only yield PASS.
//! - **ERROR ≠ vulnerability.** Execution failures are compared against a
//!   declared expectation; they are not evidence of a defect by themselves.
//! - **Completeness is not claimed.** No coverage model, no corpus, no
//!   guarantee that a campaign finds anything.
//! - **A campaign is evidence, not proof.** Findings produced from
//!   deliberately-built fixtures are known fixture behavior, not real-world
//!   vulnerability discovery.
//!
//! ## Determinism & isolation model
//!
//! - One `Host` per execution — never cloned as a snapshot.
//! - `ModuleCache` compiled once per [`Executor`], shared across cases.
//! - Baseline state is rebuilt per case; state carries forward *within* a
//!   sequence only, through explicit baseline diffing ([`sequence`]).
//! - Every case gets a fresh `Budget`.
//! - All randomness is a seeded counter PRNG keyed by
//!   `(seed, case_id, function, position)`; no global RNG state exists.
//! - Deterministic identity excludes timestamps, memory addresses, debug
//!   strings, and `ModuleCache` internals.

#![forbid(unsafe_code)]

pub mod artifact;
pub mod auth;
pub mod campaign;
mod case;
mod config;
pub mod environment;
mod error;
pub mod evidence;
mod executor;
pub mod finding;
pub mod generator;
pub mod minimizer;
pub mod mutation;
pub mod network_cost;
mod observation;
pub mod oracle;
pub mod replay;
pub mod runner;
pub mod sequence;

pub use artifact::{CampaignConfig, FindingArtifact, SCHEMA_VERSION};
pub use auth::AuthMode;
pub use campaign::{run_campaign, run_campaign_artifacts, CampaignInput, CampaignResult};
pub use case::{FunctionCall, FuzzCase};
pub use config::{BudgetConfig, FuzzConfig, LedgerConfig, PROTOCOL_VERSION};
pub use environment::{BudgetPlan, Environment};
pub use error::{FuzzError, SetupError};
pub use evidence::{CampaignEvidence, ReplayStatus};
pub use executor::Executor;
pub use finding::{CaseIdentity, Finding, SkippedFunction};
pub use generator::{GeneratedCall, GenerationCaps, GeneratorError, UnsupportedContractType};
pub use minimizer::{minimize_with_oracle, Minimization, MinimizationOutcome, MinimizationTarget};
pub use mutation::{Mutation, MutationError, Operator};
pub use observation::{
    BudgetUsage, EventRecord, ExecutionStatus, Observation, StateChange, StateEntry,
};
pub use oracle::{Classification, Expected, ExpectedBehavior, Oracle, ReasonCode};
pub use replay::{parse_artifact, replay, replay_json, ReplayError, ReplayOutcome};
pub use runner::{
    artifact_file_name, expand_seed, load_artifact, replay_line, run as run_fuzz, run_replay,
    FuzzReport, FuzzRunOptions, ReplayOptions,
};
pub use sequence::{
    apply_state_delta, execute_sequence, execute_sequence_auth, SequenceRun, SequenceState,
    StepObservation,
};
