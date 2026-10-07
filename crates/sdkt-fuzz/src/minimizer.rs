//! Deterministic minimizer.
//!
//! Strategy name (also the documentation contract for findings):
//! **"deterministically minimized under the configured strategy"** — global
//! minimality is explicitly *not* claimed.
//!
//! Reducible inputs, in this deterministic candidate order:
//! 1. sequence step removal (tail first),
//! 2. integer / bool reduction on any step's arguments (ladder per type),
//! 3. string / bytes simplification (empty),
//! 4. vec / option / map simplification (empty / None),
//! 5. environment reduction (timestamp → 0, capped budget → default).
//!
//! Preservation condition, checked by re-executing the candidate through the
//! existing [`Executor`] and re-classifying with the existing [`Oracle`]
//! (**the oracle is never mutated**):
//! - classification stays `Finding` with the **same reason code**, and
//! - error-presence semantics (`observation.is_success()`) is unchanged.
//!
//! Stopping condition: one complete pass over all candidate generators with
//! no accepted reduction.

use soroban_env_host::xdr::{LedgerEntry, ScVal};

use crate::case::FunctionCall;
use crate::environment::Environment;
use crate::error::FuzzError;
use crate::executor::Executor;
use crate::observation::Observation;
use crate::oracle::{Classification, Oracle};

/// One accepted (or rejected) reduction step, for the trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reduction {
    /// Reduction family: `step`, `arg`, `env`.
    pub kind: String,
    /// Human-readable description of the candidate (no timestamps, no
    /// debug strings — stable across runs).
    pub detail: String,
    /// Complexity after this reduction was accepted.
    pub complexity_after: usize,
}

/// Minimization outcome attached to a finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Minimization {
    /// False when minimization was not run (e.g. not a finding).
    pub attempted: bool,
    /// Always the strategy phrase — findings never claim global minima.
    pub strategy: &'static str,
    pub original_complexity: usize,
    pub minimized_complexity: usize,
    /// True when the last candidate pass preserved the finding — i.e. the
    /// result is a finding under the same reason code.
    pub preserved: bool,
    pub reductions: Vec<Reduction>,
}

impl Minimization {
    /// Identity result for cases where minimization was not run.
    pub fn not_attempted(case: &crate::case::FuzzCase) -> Self {
        let steps = std::slice::from_ref(&case.call);
        Self {
            attempted: false,
            strategy: "deterministically minimized under the configured strategy",
            original_complexity: sequence_complexity(steps),
            minimized_complexity: sequence_complexity(steps),
            preserved: false,
            reductions: Vec::new(),
        }
    }
}

/// What the minimizer executes: a call sequence, its baseline, and env.
#[derive(Clone, Debug)]
pub struct MinimizationTarget {
    /// Ordered calls; the **final** call's observation is classified.
    pub steps: Vec<FunctionCall>,
    pub baseline: Vec<LedgerEntry>,
    pub environment: Environment,
}

/// Result of a minimization run: the strategy record plus the surviving
/// step sequence.
#[derive(Clone, Debug)]
pub struct MinimizationOutcome {
    pub minimization: Minimization,
    /// The reduced step sequence (equal to the input when nothing reduced).
    pub steps: Vec<FunctionCall>,
}

/// Complexity metric: sequence length dominates, then per-argument weight.
/// Stable and cheap — the point is a deterministic ordering, not a model.
pub fn sequence_complexity(steps: &[FunctionCall]) -> usize {
    steps.len() * 10
        + steps
            .iter()
            .map(|s| s.args.iter().map(arg_complexity).sum::<usize>())
            .sum::<usize>()
}

fn arg_complexity(v: &ScVal) -> usize {
    match v {
        ScVal::Bool(_) | ScVal::Void => 1,
        ScVal::U32(n) => bits(*n as u64),
        ScVal::I32(n) => bits(*n as u64),
        ScVal::U64(n) => bits(*n),
        ScVal::I64(n) => bits(n.unsigned_abs()),
        ScVal::U128(p) => 1 + bits(p.hi) + bits(p.lo),
        ScVal::I128(p) => 1 + bits(p.hi.unsigned_abs()) + bits(p.lo),
        ScVal::String(s) => s.0.len(),
        ScVal::Bytes(b) => b.0.len(),
        ScVal::Vec(Some(v)) => 4 + v.0.iter().map(arg_complexity).sum::<usize>(),
        ScVal::Vec(None) => 1,
        ScVal::Map(Some(m)) => {
            6 + m
                .0
                .iter()
                .map(|e| arg_complexity(&e.key) + arg_complexity(&e.val))
                .sum::<usize>()
        }
        ScVal::Map(None) => 1,
        ScVal::Address(_) => 4,
        other => 8 + other.to_string_hash_ish(),
    }
}

/// Significant-bit weight: smaller magnitudes are less complex. Value-derived
/// and stable, so candidate ordering stays deterministic.
fn bits(n: u64) -> usize {
    1 + (64 - n.leading_zeros() as usize)
}

trait DebugWeight {
    fn to_string_hash_ish(&self) -> usize;
}
impl DebugWeight for ScVal {
    // Fallback weight for rare variants; length-of-Debug is stable per
    // schema version but NOT part of canonical identity — complexity only
    // orders candidates, it is never hashed.
    fn to_string_hash_ish(&self) -> usize {
        format!("{self:?}").len()
    }
}

/// Execute a step sequence against the executor's baseline and return the
/// final observation. Shared by campaign, oracle checks, and minimizer.
pub fn run_steps(
    executor: &Executor,
    baseline: &[LedgerEntry],
    environment: &Environment,
    steps: &[FunctionCall],
    case_id: &str,
) -> Result<Observation, FuzzError> {
    if steps.is_empty() {
        return Err(FuzzError::InvalidConfig(
            "minimizer/campaign sequence must have at least one step".to_string(),
        ));
    }
    // Single-call case: direct execution (the common path).
    if steps.len() == 1 {
        let case = executor.case(case_id, steps[0].clone(), baseline.to_vec());
        return executor.execute_with(&case, environment);
    }
    // Multi-step: state carries through baseline readback (Phase 2
    // sequence engine — see `sequence.rs`).
    crate::sequence::execute_sequence(executor, baseline, environment, steps, case_id)
        .map(|run| run.final_observation)
}

/// Minimize a finding's step sequence.
///
/// `original` must be the classification of the unminimized execution;
/// preservation is judged against it.
pub fn minimize(
    executor: &Executor,
    target: MinimizationTarget,
    original: &Classification,
    original_success: bool,
) -> Result<MinimizationOutcome, FuzzError> {
    let oracle = Oracle::default(); // classification of observations is done
                                    // by the *caller's* oracle; here we re-run the caller's classification
                                    // through the provided oracle below. See `minimize_with_oracle`.
    let _ = oracle;
    minimize_with_oracle(executor, target, None, original, original_success)
}

/// Minimize with an explicit oracle (the oracle rules are read, never
/// mutated). `oracle == None` reuses the plain default classifier only for
/// structure — campaign passes the real one.
pub fn minimize_with_oracle(
    executor: &Executor,
    target: MinimizationTarget,
    oracle: Option<&Oracle>,
    original: &Classification,
    original_success: bool,
) -> Result<MinimizationOutcome, FuzzError> {
    let fallback = Oracle::default();
    let oracle = oracle.unwrap_or(&fallback);

    let original_complexity = sequence_complexity(&target.steps);
    let mut current_steps = target.steps.clone();
    let mut current_complexity = original_complexity;
    let mut reductions = Vec::new();
    // Working environment: environment reduction applies in-place here so
    // `target` itself stays immutable (candidate evals read it by value).
    let mut current_env = target.environment.clone();

    let preserves = |steps: &[FunctionCall], env: &Environment| -> Result<bool, FuzzError> {
        let obs = run_steps(executor, &target.baseline, env, steps, "minimize-candidate")?;
        let classification = oracle.classify(&obs);
        Ok(classification == *original && obs.is_success() == original_success)
    };

    // A complete pass with no accepted reduction ends minimization.
    loop {
        let mut accepted_any = false;

        // 1. Step removal, tail first (deterministic order). After an
        // accepted removal the whole pass restarts via the outer loop, so
        // indices are never stale.
        if current_steps.len() > 1 {
            let mut idx = current_steps.len();
            let mut removed = false;
            while idx > 1 && !removed {
                idx -= 1;
                let mut candidate = current_steps.clone();
                candidate.remove(idx);
                if preserves(&candidate, &current_env)? {
                    let detail = format!("removed step {idx} `{}`", current_steps[idx].function);
                    current_complexity = sequence_complexity(&candidate);
                    current_steps = candidate;
                    reductions.push(Reduction {
                        kind: "step".to_string(),
                        detail,
                        complexity_after: current_complexity,
                    });
                    accepted_any = true;
                    removed = true;
                }
            }
        }

        // 2-4. Argument reductions, tail-first per step, tail-first per arg.
        for step_idx in (0..current_steps.len()).rev() {
            for arg_idx in (0..current_steps[step_idx].args.len()).rev() {
                let ladder = reduction_ladder(&current_steps[step_idx].args[arg_idx]);
                for candidate_value in ladder {
                    let mut candidate = current_steps.clone();
                    candidate[step_idx].args[arg_idx] = candidate_value.clone();
                    if preserves(&candidate, &current_env)? {
                        let detail = format!(
                            "step {step_idx} arg {arg_idx}: {} -> {}",
                            complexity_word(&current_steps[step_idx].args[arg_idx]),
                            complexity_word(&candidate_value),
                        );
                        current_complexity = sequence_complexity(&candidate);
                        current_steps = candidate;
                        reductions.push(Reduction {
                            kind: "arg".to_string(),
                            detail,
                            complexity_after: current_complexity,
                        });
                        accepted_any = true;
                        break;
                    }
                }
            }
        }

        // 5. Environment reduction (offered only when the environment is
        //    non-default; deterministic single step).
        if current_env != Environment::default() {
            let mut candidate_env = current_env.clone();
            candidate_env.ledger.timestamp = 0;
            if let crate::environment::BudgetPlan::Capped { .. } = candidate_env.budget {
                candidate_env.budget = crate::environment::BudgetPlan::Default;
            }
            if candidate_env != current_env && preserves(&current_steps, &candidate_env)? {
                reductions.push(Reduction {
                    kind: "env".to_string(),
                    detail: "environment reduced to defaults".to_string(),
                    complexity_after: current_complexity,
                });
                current_env = candidate_env;
                accepted_any = true;
            }
        }

        if !accepted_any {
            break;
        }
    }

    // Verify the final sequence still preserves (it does by construction,
    // but state the result from an actual run — no assumption).
    let final_obs = run_steps(
        executor,
        &target.baseline,
        &current_env,
        &current_steps,
        "minimize-final",
    )?;
    let preserved =
        oracle.classify(&final_obs) == *original && final_obs.is_success() == original_success;

    Ok(MinimizationOutcome {
        minimization: Minimization {
            attempted: true,
            strategy: "deterministically minimized under the configured strategy",
            original_complexity,
            minimized_complexity: sequence_complexity(&current_steps),
            preserved,
            reductions,
        },
        steps: current_steps,
    })
}

/// Deterministic reduction ladder for one value, simplest-first.
fn reduction_ladder(v: &ScVal) -> Vec<ScVal> {
    match v {
        ScVal::Bool(b) => {
            if *b {
                vec![ScVal::Bool(false)]
            } else {
                vec![]
            }
        }
        ScVal::U32(x) if *x != 0 => vec![ScVal::U32(0)],
        ScVal::I32(x) if *x != 0 => vec![ScVal::I32(0)],
        ScVal::U64(x) if *x != 0 => vec![ScVal::U64(0)],
        ScVal::I64(x) if *x != 0 => vec![ScVal::I64(0)],
        ScVal::U128(p) if p.hi != 0 || p.lo != 0 => {
            vec![ScVal::U128(soroban_env_host::xdr::UInt128Parts {
                hi: 0,
                lo: 0,
            })]
        }
        ScVal::I128(p) if p.hi != 0 || p.lo != 0 => {
            vec![ScVal::I128(soroban_env_host::xdr::Int128Parts {
                hi: 0,
                lo: 0,
            })]
        }
        ScVal::U256(p) if p.hi_hi != 0 || p.hi_lo != 0 || p.lo_hi != 0 || p.lo_lo != 0 => {
            vec![ScVal::U256(soroban_env_host::xdr::UInt256Parts {
                hi_hi: 0,
                hi_lo: 0,
                lo_hi: 0,
                lo_lo: 0,
            })]
        }
        ScVal::I256(p) if p.hi_hi != 0 || p.hi_lo != 0 || p.lo_hi != 0 || p.lo_lo != 0 => {
            vec![ScVal::I256(soroban_env_host::xdr::Int256Parts {
                hi_hi: 0,
                hi_lo: 0,
                lo_hi: 0,
                lo_lo: 0,
            })]
        }
        ScVal::String(s) if !s.0.is_empty() => {
            vec![ScVal::String(soroban_env_host::xdr::ScString(
                soroban_env_host::xdr::StringM::default(),
            ))]
        }
        ScVal::Bytes(b) if !b.0.is_empty() => {
            vec![ScVal::Bytes(soroban_env_host::xdr::ScBytes(
                soroban_env_host::xdr::BytesM::default(),
            ))]
        }
        ScVal::Vec(Some(v)) if !v.0.is_empty() => vec![ScVal::Vec(None)],
        ScVal::Map(Some(m)) if !m.0.is_empty() => vec![ScVal::Map(None)],
        ScVal::Address(_) => vec![], // addresses are not reducible safely
        _ => vec![],
    }
}

fn complexity_word(v: &ScVal) -> String {
    match v {
        ScVal::Bool(b) => format!("bool({b})"),
        ScVal::U32(x) => format!("u32({x})"),
        ScVal::I32(x) => format!("i32({x})"),
        ScVal::U64(x) => format!("u64({x})"),
        ScVal::I64(x) => format!("i64({x})"),
        ScVal::Void => "void".to_string(),
        ScVal::String(s) => format!("string(len {})", s.0.len()),
        ScVal::Bytes(b) => format!("bytes(len {})", b.0.len()),
        ScVal::Vec(Some(v)) => format!("vec(len {})", v.0.len()),
        ScVal::Vec(None) => "vec(empty)".to_string(),
        ScVal::Map(Some(m)) => format!("map(len {})", m.0.len()),
        ScVal::Map(None) => "map(empty)".to_string(),
        ScVal::Address(_) => "address".to_string(),
        _ => "value".to_string(),
    }
}
