//! The explicit oracle: PASS / EXPECTED_ERROR / FINDING.
//!
//! Core rule: **ERROR ≠ vulnerability.** An execution failure is classified
//! only against a *declared* [`ExpectedBehavior`] — never inferred from the
//! error's shape, and never automatically a finding.
//!
//! Findable conditions and their exact triggers:
//!
//! - [`ReasonCode::ReturnMismatch`] — declared [`Expected::Success`] with an
//!   expected return value, and the call succeeded with a different value.
//! - [`ReasonCode::StateMismatch`] — declared expected state differs from
//!   the observed state (byte-exact XDR comparison).
//! - [`ReasonCode::EventMismatch`] — declared expected events differ from
//!   the observed events.
//! - [`ReasonCode::UnexpectedError`] — declared `Success` (or `SuccessWith`)
//!   and the execution produced an error.
//! - [`ReasonCode::AuthorizationBypass`] — only when the oracle declares
//!   `auth_required = true` AND the host enforced authorization AND the
//!   execution succeeded anyway.
//! - [`ReasonCode::ResourceLimit`] — only when declared [`Expected::Success`]
//!   is met with a *budget-class* host error **and** the oracle's
//!   `resource_limit_is_finding` rule is explicitly enabled. Otherwise a
//!   budget error under `Expected::Any` is `ExpectedError`, and under
//!   `Success` without the rule it is `UnexpectedError` — the mapping to
//!   RESOURCE_LIMIT is never automatic.
//!
//! Everything else is [`Classification::Pass`] or
//! [`Classification::ExpectedError`].

use soroban_env_host::xdr::ScVal;

use crate::observation::{ExecutionStatus, Observation, StateChange, StateEntry};

/// Declared expectation for one case. No inference: absent fields mean
/// "not declared", and undeclared conditions can never produce findings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expected {
    /// Declared call outcome.
    pub behavior: ExpectedBehavior,
    /// Declared post-execution state (encoded-key XDR → encoded-entry XDR).
    /// `None` = state not checked.
    pub state: Option<std::collections::BTreeMap<Vec<u8>, Vec<u8>>>,
    /// Declared expected events. `None` = events not checked.
    pub events: Option<Vec<crate::observation::EventRecord>>,
    /// Declared: this function requires authorization to succeed. Only when
    /// `true` can a success-without-auth observation become an
    /// AUTHORIZATION_BYPASS finding.
    pub auth_required: bool,
    /// Declared rule: map budget-class failures to RESOURCE_LIMIT findings.
    /// Default off — resource exhaustion is an outcome, not a verdict.
    pub resource_limit_is_finding: bool,
}

/// Declared call outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExpectedBehavior {
    /// Must succeed; optionally the return value must equal this.
    Success { expect_return: Option<ScVal> },
    /// Must fail with exactly this (error_type, code) pair.
    Error { error_type: String, code: u32 },
    /// Outcome unchecked.
    #[default]
    Any,
}

/// Reason a finding was raised. Stable names; the *absence* of a reason for
/// an error is by design (ERROR ≠ vulnerability).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReasonCode {
    ReturnMismatch,
    StateMismatch,
    EventMismatch,
    AuthorizationBypass,
    UnexpectedError,
    ResourceLimit,
}

impl ReasonCode {
    /// Inverse of [`ReasonCode::name`]; `None` for unknown names.
    /// Single source of the reason vocabulary (artifact/replay reuse it).
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "RETURN_MISMATCH" => Some(ReasonCode::ReturnMismatch),
            "STATE_MISMATCH" => Some(ReasonCode::StateMismatch),
            "EVENT_MISMATCH" => Some(ReasonCode::EventMismatch),
            "AUTHORIZATION_BYPASS" => Some(ReasonCode::AuthorizationBypass),
            "UNEXPECTED_ERROR" => Some(ReasonCode::UnexpectedError),
            "RESOURCE_LIMIT" => Some(ReasonCode::ResourceLimit),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ReasonCode::ReturnMismatch => "RETURN_MISMATCH",
            ReasonCode::StateMismatch => "STATE_MISMATCH",
            ReasonCode::EventMismatch => "EVENT_MISMATCH",
            ReasonCode::AuthorizationBypass => "AUTHORIZATION_BYPASS",
            ReasonCode::UnexpectedError => "UNEXPECTED_ERROR",
            ReasonCode::ResourceLimit => "RESOURCE_LIMIT",
        }
    }
}

/// Oracle verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Classification {
    /// Outcome matched the declaration (or nothing was declared).
    Pass,
    /// Outcome matched a declared error expectation.
    ExpectedError,
    /// An explicitly-declared rule was violated.
    Finding(ReasonCode),
}

/// The oracle: classifies an [`Observation`] against a declared [`Expected`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Oracle {
    pub expected: Expected,
}

/// Budget-class host error: the host's Budget error type.
pub(crate) fn is_budget_error(error_type: &str) -> bool {
    error_type == "Budget"
}

impl Oracle {
    pub fn new(expected: Expected) -> Self {
        Self { expected }
    }

    /// Classify one observation. Deterministic pure function.
    pub fn classify(&self, obs: &Observation) -> Classification {
        let expected_error = matches!(self.expected.behavior, ExpectedBehavior::Error { .. });

        // --- Call outcome checks -------------------------------------
        match &obs.status {
            ExecutionStatus::ContractError { error_type, code } => {
                if let ExpectedBehavior::Error {
                    error_type: want_type,
                    code: want_code,
                } = &self.expected.behavior
                {
                    if want_type == error_type && want_code == code {
                        return Classification::ExpectedError;
                    }
                    // Declared a specific error but got a different one:
                    // the declaration was violated. UnexpectedError.
                    return Classification::Finding(ReasonCode::UnexpectedError);
                }
                if matches!(self.expected.behavior, ExpectedBehavior::Any) {
                    return Classification::Pass;
                }
                // Declared Success (with or without a return value).
                if self.expected.resource_limit_is_finding && is_budget_error(error_type) {
                    return Classification::Finding(ReasonCode::ResourceLimit);
                }
                return Classification::Finding(ReasonCode::UnexpectedError);
            }
            ExecutionStatus::Void | ExecutionStatus::Returned(_) => {
                if expected_error {
                    // Declared an error but the call produced a result: the
                    // outcome differs from the declared expectation. Not
                    // UNEXPECTED_ERROR — that reason code is reserved (per
                    // contract) for errors raised where Success was declared.
                    return Classification::Finding(ReasonCode::ReturnMismatch);
                }
                if matches!(self.expected.behavior, ExpectedBehavior::Any) {
                    // Outcome unchecked, but state/events/auth checks below
                    // still apply when declared.
                } else if let ExpectedBehavior::Success {
                    expect_return: Some(want),
                } = &self.expected.behavior
                {
                    let got = obs.return_value().cloned().unwrap_or(ScVal::Void);
                    if got != *want {
                        return Classification::Finding(ReasonCode::ReturnMismatch);
                    }
                }
            }
        }

        // --- Authorization bypass ------------------------------------
        // Only when: declared auth_required AND host enforced AND success.
        // (This oracle instance classifies a NoAuth execution; the campaign
        // wires Correct/WrongAuth runs through ExpectedError declarations.)
        if self.expected.auth_required && obs.is_success() {
            return Classification::Finding(ReasonCode::AuthorizationBypass);
        }

        // --- State check ----------------------------------------------
        if let Some(want_state) = &self.expected.state {
            for entry in &obs.state {
                let expected_value = want_state.get(&entry.key_xdr);
                let mismatch = match (&entry.change, expected_value) {
                    (StateChange::Deleted, Some(_)) => true,
                    (StateChange::Deleted, None) => false,
                    (_, None) => {
                        // Not declared: only a *creation* we didn't declare
                        // counts as a mismatch.
                        entry.change == StateChange::Created
                    }
                    (_, Some(want)) => entry.value_xdr.as_deref() != Some(want.as_slice()),
                };
                if mismatch {
                    return Classification::Finding(ReasonCode::StateMismatch);
                }
            }
            // Every declared entry must appear.
            for (key, value) in want_state {
                let found = obs
                    .state
                    .iter()
                    .any(|e| e.key_xdr == *key && e.value_xdr.as_deref() == Some(value.as_slice()));
                if !found {
                    return Classification::Finding(ReasonCode::StateMismatch);
                }
            }
        }

        // --- Event check -----------------------------------------------
        if let Some(want_events) = &self.expected.events {
            if &obs.events != want_events {
                return Classification::Finding(ReasonCode::EventMismatch);
            }
        }

        Classification::Pass
    }
}

/// Helper: the set of changed keys in an observation (for state expectations).
pub fn changed_keys(obs: &Observation) -> Vec<&StateEntry> {
    obs.state
        .iter()
        .filter(|e| e.change != StateChange::Unchanged)
        .collect()
}
