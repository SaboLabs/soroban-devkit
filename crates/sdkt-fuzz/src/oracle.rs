//! The explicit oracle: PASS / EXPECTED_ERROR / FINDING.
//!
//! Core rule: **ERROR ≠ vulnerability.** An execution failure is classified
//! only against a *declared* [`ExpectedBehavior`] — never inferred from the
//! error's shape, and never automatically a finding.
//!
//! Findable conditions and their exact triggers:
//!
//! - [`ReasonCode::ReturnMismatch`] — the call's outcome differs from the
//!   declared expectation in the *return* direction: declared
//!   [`Expected::Success`] with an expected return value, and the call
//!   succeeded with a different value — or declared an error, and the call
//!   succeeded instead of failing.
//! - [`ReasonCode::StateMismatch`] — declared expected state differs from
//!   the observed state (byte-exact XDR comparison).
//! - [`ReasonCode::EventMismatch`] — declared expected events differ from
//!   the observed events.
//! - [`ReasonCode::UnexpectedError`] — the execution produced an error the
//!   declaration did not anticipate: declared `Success` (or `SuccessWith`)
//!   and the execution produced an error — or declared a specific error
//!   (`Error { type, code }`) and the execution produced a *different*
//!   error.
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
///
/// `auth_mode` is the authorization posture the classified execution ran
/// under. It is only consulted by the AUTHORIZATION_BYPASS rule (an
/// authorized success is not a bypass); every other rule is mode-independent.
/// The default ([`AuthMode::NoAuth`]) preserves the historical semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Oracle {
    pub expected: Expected,
    pub auth_mode: crate::auth::AuthMode,
}

/// Budget-class host error: the host's Budget error type.
pub(crate) fn is_budget_error(error_type: &str) -> bool {
    error_type == "Budget"
}

impl Oracle {
    /// Oracle for an execution that ran without authorization entries.
    pub fn new(expected: Expected) -> Self {
        Self {
            expected,
            auth_mode: crate::auth::AuthMode::NoAuth,
        }
    }

    /// Oracle for an execution that ran under an explicit [`AuthMode`].
    pub fn with_auth_mode(expected: Expected, auth_mode: crate::auth::AuthMode) -> Self {
        Self {
            expected,
            auth_mode,
        }
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
                    // the execution produced an error the declaration did
                    // not anticipate. UnexpectedError.
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
                    // outcome differs from the declared expectation in the
                    // return direction. ReturnMismatch — not
                    // UNEXPECTED_ERROR, which is reserved for errors the
                    // declaration did not anticipate (and no error was
                    // raised here at all).
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
        // Declared auth_required + success + the case was NOT authorized with
        // a matching credential.
        if self.expected.auth_required
            && obs.is_success()
            && self.auth_mode != crate::auth::AuthMode::CorrectAuth
        {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::BudgetUsage;

    fn obs(status: ExecutionStatus) -> Observation {
        Observation {
            case_id: "case0000".to_string(),
            function: "hello".to_string(),
            status,
            state: vec![],
            events: vec![],
            budget: BudgetUsage::default(),
        }
    }

    fn declared_error(error_type: &str, code: u32) -> Expected {
        Expected {
            behavior: ExpectedBehavior::Error {
                error_type: error_type.to_string(),
                code,
            },
            ..Default::default()
        }
    }

    /// Issue #367, case 1: a declared error that receives a *different*
    /// error is an error the declaration did not anticipate —
    /// UNEXPECTED_ERROR, not a silent match.
    #[test]
    fn declared_error_receiving_different_error_is_unexpected_error() {
        let oracle = Oracle::new(declared_error("Contract", 1));
        let observed = obs(ExecutionStatus::ContractError {
            error_type: "Auth".to_string(),
            code: 6,
        });
        assert_eq!(
            oracle.classify(&observed),
            Classification::Finding(ReasonCode::UnexpectedError)
        );
    }

    /// Issue #367, case 2: a declared error that succeeds is a return
    /// mismatch — the call produced a result where the declaration said it
    /// must fail.
    #[test]
    fn declared_error_that_succeeds_is_return_mismatch() {
        let oracle = Oracle::new(declared_error("Auth", 1));
        let observed = obs(ExecutionStatus::Returned(ScVal::U32(42)));
        assert_eq!(
            oracle.classify(&observed),
            Classification::Finding(ReasonCode::ReturnMismatch)
        );
        // A void success is the same outcome direction.
        let observed = obs(ExecutionStatus::Void);
        assert_eq!(
            oracle.classify(&observed),
            Classification::Finding(ReasonCode::ReturnMismatch)
        );
    }

    /// Control: a declared error that matches the actual error is
    /// EXPECTED_ERROR, never a finding.
    #[test]
    fn declared_error_matching_is_expected_error() {
        let oracle = Oracle::new(declared_error("Auth", 6));
        let observed = obs(ExecutionStatus::ContractError {
            error_type: "Auth".to_string(),
            code: 6,
        });
        assert_eq!(oracle.classify(&observed), Classification::ExpectedError);
    }

    /// Control: the documented ReturnMismatch case — declared Success with
    /// an expected return value, call succeeded with a different value.
    #[test]
    fn declared_success_with_wrong_return_is_return_mismatch() {
        let oracle = Oracle::new(Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: Some(ScVal::U32(43)),
            },
            ..Default::default()
        });
        let observed = obs(ExecutionStatus::Returned(ScVal::U32(42)));
        assert_eq!(
            oracle.classify(&observed),
            Classification::Finding(ReasonCode::ReturnMismatch)
        );
    }

    /// Control: the documented UnexpectedError case — declared Success,
    /// execution produced an error.
    #[test]
    fn declared_success_that_errors_is_unexpected_error() {
        let oracle = Oracle::new(Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: None,
            },
            ..Default::default()
        });
        let observed = obs(ExecutionStatus::ContractError {
            error_type: "Contract".to_string(),
            code: 1,
        });
        assert_eq!(
            oracle.classify(&observed),
            Classification::Finding(ReasonCode::UnexpectedError)
        );
    }
}
