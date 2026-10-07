//! Deterministic replay from a finding artifact (Phase 3).
//!
//! Replay is the verification half of the pipeline: it takes an artifact
//! plus the campaign WASM and re-runs **the exact same execution** through
//! the public Soroban host API, then re-applies the same oracle rules and
//! compares the outcome.
//!
//! Failure taxonomy (kept strictly separate):
//!
//! - **Invalid input** → [`ReplayError`]: unknown schema version, malformed
//!   artifact, or a WASM whose SHA-256 does not match the artifact. These are
//!   *never* a [`crate::Classification::Finding`] and never a contract result.
//! - **Reproduced** → [`ReplayOutcome::Reproduced`]: the re-execution's
//!   observation still classifies the same way, with the same reason code.
//! - **Mismatch** → [`ReplayOutcome::Mismatch`]: the artifact is well-formed
//!   and the WASM matches, but the re-execution does not reproduce the
//!   recorded classification. A mismatch is a *replay* verdict, not a
//!   security conclusion about the contract.
//!
//! No host internals, `testutils`, or `recording_mode` are used: replay goes
//! through [`crate::Executor`], which owns the public execution path.

use sha2::{Digest, Sha256};

use crate::artifact::{FindingArtifact, SCHEMA_VERSION};
use crate::auth::AuthMode;
use crate::environment::Environment;
use crate::executor::Executor;
use crate::observation::Observation;
use crate::oracle::{Classification, Oracle, ReasonCode};
use crate::sequence::execute_sequence;

/// Replay could not be carried out — invalid input, never a finding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    #[error("artifact JSON could not be parsed: {0}")]
    Malformed(String),
    #[error("unsupported artifact schema_version {found} (this build knows {expected})")]
    UnsupportedSchema { found: u32, expected: u32 },
    #[error("artifact did not round-trip: {0}")]
    InvalidArtifact(String),
    #[error("wasm hash mismatch: artifact={expected} supplied={actual}")]
    WasmHashMismatch { expected: String, actual: String },
    #[error("execution could not be performed: {0}")]
    Execution(String),
}

/// Replay verdict for a well-formed artifact against a matching WASM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayOutcome {
    /// The recorded classification and reason reproduced exactly.
    Reproduced {
        reason_code: ReasonCode,
        observation_hash: String,
        /// The artifact's canonical hash (identity of the replayed input).
        artifact_hash: String,
    },
    /// The artifact is valid but the re-execution disagreed.
    Mismatch {
        expected: ReasonCode,
        actual: Classification,
        observation_hash: String,
        artifact_hash: String,
    },
}

impl ReplayOutcome {
    /// True only for a reproduced finding.
    pub fn is_reproduced(&self) -> bool {
        matches!(self, ReplayOutcome::Reproduced { .. })
    }

    /// SHA-256 of this outcome's canonical description — the deterministic
    /// cross-process comparison value.
    pub fn canonical_hash(&self) -> String {
        let mut h = Sha256::new();
        match self {
            ReplayOutcome::Reproduced {
                reason_code,
                observation_hash,
                artifact_hash,
            } => {
                h.update(b"reproduced");
                h.update(reason_code.name().as_bytes());
                h.update(observation_hash.as_bytes());
                h.update(artifact_hash.as_bytes());
            }
            ReplayOutcome::Mismatch {
                expected,
                actual,
                observation_hash,
                artifact_hash,
            } => {
                h.update(b"mismatch");
                h.update(expected.name().as_bytes());
                h.update(format!("{actual:?}").as_bytes());
                h.update(observation_hash.as_bytes());
                h.update(artifact_hash.as_bytes());
            }
        }
        hex(&h.finalize())
    }
}

/// Parse an artifact from JSON bytes, validating the schema version.
pub fn parse_artifact(json: &[u8]) -> Result<FindingArtifact, ReplayError> {
    let artifact: FindingArtifact =
        serde_json::from_slice(json).map_err(|e| ReplayError::Malformed(e.to_string()))?;
    if artifact.schema_version != SCHEMA_VERSION {
        return Err(ReplayError::UnsupportedSchema {
            found: artifact.schema_version,
            expected: SCHEMA_VERSION,
        });
    }
    Ok(artifact)
}

/// Verify that `wasm` is the artifact's WASM. Returns the hex digest on
/// match. A mismatch is invalid input (never a finding, never a contract
/// result).
pub fn verify_wasm(artifact: &FindingArtifact, wasm: &[u8]) -> Result<String, ReplayError> {
    let actual = hex(&Sha256::digest(wasm));
    if !artifact.campaign.wasm_sha256.eq_ignore_ascii_case(&actual) {
        return Err(ReplayError::WasmHashMismatch {
            expected: artifact.campaign.wasm_sha256.clone(),
            actual,
        });
    }
    Ok(actual)
}

/// Replay a finding artifact against its WASM.
///
/// Steps: validate schema → validate WASM hash → rebuild the execution
/// context from the artifact → execute through the public host path → apply
/// the recorded oracle rules → compare with the recorded reason code.
pub fn replay(artifact: &FindingArtifact, wasm: &[u8]) -> Result<ReplayOutcome, ReplayError> {
    verify_wasm(artifact, wasm)?;

    if artifact.schema_version != SCHEMA_VERSION {
        return Err(ReplayError::UnsupportedSchema {
            found: artifact.schema_version,
            expected: SCHEMA_VERSION,
        });
    }

    let expected_reason = ReasonCode::from_name(&artifact.reason_code).ok_or_else(|| {
        ReplayError::InvalidArtifact(format!("unknown reason_code `{}`", artifact.reason_code))
    })?;
    let steps = artifact
        .steps()
        .map_err(|e| ReplayError::InvalidArtifact(e.to_string()))?;
    if steps.is_empty() {
        return Err(ReplayError::InvalidArtifact(
            "artifact contains no sequence steps".to_string(),
        ));
    }
    let environment: Environment = artifact.environment();
    let seed = artifact
        .seed()
        .map_err(|e| ReplayError::InvalidArtifact(e.to_string()))?;

    // Rebuild the campaign execution context from the artifact alone.
    let mut config = crate::config::FuzzConfig::default();
    config.seed = seed;
    config.network_id = crate::config::FuzzConfig::default().network_id;

    let executor =
        Executor::new(wasm, config).map_err(|e| ReplayError::Execution(e.to_string()))?;

    // Auth entries: rebuilt from the artifact's declared mode. The root
    // invocation is the artifact's first step (the campaign executes one
    // call per case). Modes the public API cannot exercise faithfully are
    // refused explicitly rather than approximated.
    let auth_entries = if artifact.auth_mode == AuthMode::NoAuth.name() {
        Vec::new()
    } else {
        let mode = AuthMode::from_name(&artifact.auth_mode).ok_or_else(|| {
            ReplayError::InvalidArtifact(format!("unknown auth_mode `{}`", artifact.auth_mode))
        })?;
        let contract = executor
            .case(&artifact.case_id, steps[0].clone(), Vec::new())
            .contract_address();
        crate::auth::invoke_auth_entries(mode, &contract, &steps[0])
            .map_err(|e| ReplayError::Execution(e.to_string()))?
    };

    // Execute exactly what the artifact describes, through the public path.
    let observation: Observation = if steps.len() > 1 {
        execute_sequence(&executor, &[], &environment, &steps, &artifact.case_id)
            .map_err(|e| ReplayError::Execution(e.to_string()))?
            .final_observation
    } else {
        let case = executor.case(&artifact.case_id, steps[0].clone(), Vec::new());
        executor
            .execute_with_auth(&case, &environment, &auth_entries)
            .map_err(|e| ReplayError::Execution(e.to_string()))?
    };

    // Re-apply the recorded oracle rules.
    let oracle = Oracle::new(artifact.expected());
    let actual = oracle.classify(&observation);
    let observation_hash = observation_hash(&observation);
    let artifact_hash = artifact.canonical_hash();

    Ok(match actual {
        Classification::Finding(reason) if reason == expected_reason => ReplayOutcome::Reproduced {
            reason_code: reason,
            observation_hash,
            artifact_hash,
        },
        other => ReplayOutcome::Mismatch {
            expected: expected_reason,
            actual: other,
            observation_hash,
            artifact_hash,
        },
    })
}

/// Convenience: parse + replay in one call.
pub fn replay_json(json: &[u8], wasm: &[u8]) -> Result<ReplayOutcome, ReplayError> {
    let artifact = parse_artifact(json)?;
    replay(&artifact, wasm)
}

/// Deterministic digest of an observation's *value* content.
///
/// Excludes nothing that matters and includes nothing that does not: the
/// status, footprint state, events and budget counters, each encoded
/// canonically. No debug strings, no host internals, no pointers.
pub fn observation_hash(obs: &Observation) -> String {
    let snapshot = crate::artifact::ObservationSnapshot::from_observation(obs);
    let value = serde_json::to_value(&snapshot).expect("observation snapshot serializes");
    let canonical = canonical_sorted(value);
    hex(&Sha256::digest(
        serde_json::to_vec(&canonical).expect("canonical json"),
    ))
}

fn canonical_sorted(value: serde_json::Value) -> serde_json::Value {
    use serde_json::{Map, Value};
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (k, v) in entries {
                out.insert(k, canonical_sorted(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical_sorted).collect()),
        other => other,
    }
}

fn hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 0x0f) as usize] as char);
    }
    s
}
