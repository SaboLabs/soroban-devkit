//! Typed error taxonomy for the fuzz execution core.
//!
//! These are **not** classification results. Nothing here means
//! "vulnerability": a [`FuzzError`] is a usage, setup, execution, or observation
//! failure. Classifying execution outcomes as FINDING / EXPECTED_ERROR is an
//! oracle concern and is out of scope for this crate.

/// Why a fuzz operation could not be carried out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FuzzError {
    /// Caller passed an unusable configuration (e.g. `cases == 0`).
    #[error("invalid fuzz configuration: {0}")]
    InvalidConfig(String),

    /// The WASM bytes or the baseline ledger setup were unusable.
    #[error("invalid WASM or baseline setup: {0}")]
    InvalidSetup(#[from] SetupError),

    /// The Soroban host could not be constructed for a case.
    #[error("host construction failed: {0}")]
    HostConstruction(String),

    /// A case could not be executed at all (as opposed to returning an
    /// error value from the contract, which is an [`Observation`]).
    #[error("execution failed: {0}")]
    Execution(String),

    /// A result could not be turned into a stable [`crate::Observation`].
    #[error("observation failed: {0}")]
    Observation(String),

    /// A finding artifact is malformed, has an unknown schema version, or
    /// fails replay validation (e.g. the WASM hash does not match). An
    /// invalid artifact or wrong WASM is an *input* problem, never a
    /// security finding and never a contract result.
    #[error("invalid artifact: {0}")]
    InvalidArtifact(String),
}

/// Problems with the decoded inputs handed to the executor.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetupError {
    /// WASM bytes empty, or rejected by the Soroban engine.
    #[error("wasm rejected: {0}")]
    Wasm(String),

    /// A baseline ledger entry could not be decoded.
    #[error("baseline ledger entry rejected: {0}")]
    BaselineEntry(String),
}
