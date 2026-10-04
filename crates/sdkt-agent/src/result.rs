//! Structured result document produced by the agent.
//!
//! The schema is deliberately explicit about *what ran* and *what it
//! returned*, so an orchestrator never has to re-read the request to learn
//! whether a number came from a verdict or from a crash:
//!
//! - [`Status`] is the single truth for success/failure/refusal.
//! - `exit_code` is reported verbatim, including `None` on timeout.
//! - `stdout_json` is populated only when the registry said JSON exists *and*
//!   stdout actually parsed; `stdout_text` keeps the raw stream either way.
//! - `stderr` is diagnostic evidence and is never merged into stdout.
//! - [`Evidence`] separates "the command ran" from "here is what sdkt said"
//!   from "here is the diagnostic noise".

use serde::{Deserialize, Serialize};

use crate::executor::RawExecution;
use crate::plan::{Plan, PlanError};
use crate::AGENT_SCHEMA_VERSION;

/// Outcome of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Ran and reported a non-failing verdict.
    Success,
    /// Ran, but the command failed or its verdict failed.
    Failed,
    /// Not run: the request was ambiguous, unrecognised, or missing arguments.
    NeedsClarification,
    /// Not run: refused on safety or network grounds.
    Blocked,
}

/// Provenance for each claim in the result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// The exact command line that was executed.
    pub command: String,
    /// Exit code reported by the child process; `None` on timeout/spawn error.
    pub exit_code: Option<i32>,
    /// True when a process was actually spawned and awaited.
    pub command_executed: bool,
    /// True when the process exceeded the timeout and was killed.
    pub timed_out: bool,
    /// True when stdout parsed as JSON (only possible when JSON was requested).
    pub parsed_sdkt_result: bool,
    /// stderr, kept as a separate diagnostic channel.
    pub stderr: String,
}

/// The agent's output document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Schema version of this document.
    pub schema_version: u32,
    /// The original user request.
    pub request: String,
    /// Registry capability id, when one was resolved.
    pub capability_id: Option<String>,
    /// Human-facing command name (`sdkt health`), empty when refused.
    pub command: String,
    /// Full argv, empty when refused.
    pub argv: Vec<String>,
    /// Network used, when one applies.
    pub network: Option<String>,
    /// Registry safety class, when a capability was resolved.
    pub safety: Option<String>,
    /// Whether JSON output was requested.
    pub json_requested: bool,
    /// Single truth for the outcome.
    pub status: Status,
    /// Exit code from the child process, when one ran.
    pub exit_code: Option<i32>,
    /// Parsed stdout JSON, when available and parseable.
    pub stdout_json: Option<serde_json::Value>,
    /// Raw stdout, truncated for readability.
    pub stdout_text: Option<String>,
    /// Provenance detail.
    pub evidence: Evidence,
    /// Why this status, in one or two sentences.
    pub explanation: String,
    /// Machine-stable refusal code, when the request was not run.
    pub error_code: Option<String>,
}

impl ExecutionResult {
    /// Build the refusal result for a request that was never executed.
    pub fn refused(request: &str, error: &PlanError) -> Self {
        let status = match error {
            PlanError::Blocked { .. } | PlanError::RefusedNetwork(_) => Status::Blocked,
            _ => Status::NeedsClarification,
        };
        ExecutionResult {
            schema_version: AGENT_SCHEMA_VERSION,
            request: request.to_string(),
            capability_id: match error {
                PlanError::Blocked { capability_id, .. } => Some((*capability_id).to_string()),
                PlanError::Parse(crate::intent::ParseError::MutationRequested {
                    capability_id,
                }) => Some((*capability_id).to_string()),
                _ => None,
            },
            command: String::new(),
            argv: Vec::new(),
            network: None,
            safety: None,
            json_requested: false,
            status,
            exit_code: None,
            stdout_json: None,
            stdout_text: None,
            evidence: Evidence::default(),
            explanation: error.message(),
            error_code: Some(error.code().to_string()),
        }
    }

    /// Build a result from a plan plus its raw execution.
    pub fn from_execution(plan: &Plan, raw: &RawExecution) -> Self {
        let command_executed = raw.exit_code.is_some() || raw.timed_out;
        let code = raw.exit_code;

        // A verdict-gated capability can exit non-zero while still printing a
        // perfectly good report; that is a `Failed` verdict, not a crash.
        let cap = sdkt_core::registry::find(plan.capability_id);
        let verdict_gated = cap.is_some_and(|c| c.exit.verdict_can_fail);

        let status = match code {
            None => Status::Failed, // timeout or spawn failure
            Some(0) => Status::Success,
            Some(_) => Status::Failed,
        };

        let stdout_json = if plan.wants_json && !raw.timed_out && !raw.spawn_failed {
            serde_json::from_str::<serde_json::Value>(&raw.stdout).ok()
        } else {
            None
        };

        let explanation = explain(status, raw, stdout_json.as_ref(), verdict_gated);
        let parsed_sdkt_result = stdout_json.is_some();

        ExecutionResult {
            schema_version: AGENT_SCHEMA_VERSION,
            request: String::new(), // filled in by the caller that owns the request
            capability_id: Some(plan.capability_id.to_string()),
            command: format!("sdkt {}", plan.argv.join(" ")),
            argv: plan.argv.clone(),
            network: plan.network.clone(),
            safety: Some(format!("{:?}", plan.safety)),
            json_requested: plan.wants_json,
            status,
            exit_code: code,
            stdout_json,
            stdout_text: Some(truncate(&raw.stdout)),
            evidence: Evidence {
                command: format!("sdkt {}", plan.argv.join(" ")),
                exit_code: code,
                command_executed,
                timed_out: raw.timed_out,
                parsed_sdkt_result,
                stderr: truncate(&raw.stderr),
            },
            explanation,
            error_code: None,
        }
    }

    /// Serialise as pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    }

    /// Short human summary.
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("REQUEST     {}\n", self.request));
        out.push_str(&format!(
            "CAPABILITY  {}\n",
            self.capability_id.as_deref().unwrap_or("(none)")
        ));
        out.push_str(&format!(
            "COMMAND     {}\n",
            if self.command.is_empty() {
                "(not executed)"
            } else {
                &self.command
            }
        ));
        out.push_str(&format!(
            "RESULT      {:?} (exit {})\n",
            self.status,
            self.exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ));
        out.push_str(&format!(
            "EVIDENCE    executed={} json={} stderr={}B\n",
            self.evidence.command_executed,
            self.evidence.parsed_sdkt_result,
            self.evidence.stderr.len()
        ));
        out.push_str(&format!("EXPLANATION  {}", self.explanation));
        out
    }
}

fn explain(
    status: Status,
    raw: &RawExecution,
    json: Option<&serde_json::Value>,
    verdict_gated: bool,
) -> String {
    if raw.spawn_failed {
        return "the sdkt binary could not be started".to_string();
    }
    if raw.timed_out {
        return "the command exceeded the agent's timeout and was killed".to_string();
    }
    match (status, raw.exit_code) {
        (Status::Success, Some(0)) => match json {
            Some(_) => {
                "the read-only check completed and sdkt returned a structured result"
                    .to_string()
            }
            None => "the read-only check completed successfully".to_string(),
        },
        (Status::Failed, Some(c)) if verdict_gated => format!(
            "sdkt ran to completion but the verdict failed (exit {c}); the report above is the evidence"
        ),
        (Status::Failed, Some(c)) => {
            format!("the command failed (exit {c}); see stderr for the diagnostic")
        }
        _ => "the command did not complete".to_string(),
    }
}

fn truncate(s: &str) -> String {
    const MAX: usize = 8 * 1024;
    if s.len() <= MAX {
        s.to_string()
    } else {
        let mut end = MAX;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n…[truncated]", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plan() -> Plan {
        Plan {
            capability_id: "health",
            argv: vec![
                "health".into(),
                "--contract".into(),
                "CABC".into(),
                "--format".into(),
                "json".into(),
            ],
            network: Some("testnet".into()),
            safety: sdkt_core::registry::Safety::ReadOnly,
            wants_json: true,
            matched_keywords: vec!["health".into()],
        }
    }

    #[test]
    fn success_with_json_is_success() {
        let raw = RawExecution {
            exit_code: Some(0),
            stdout: r#"{"health":"healthy"}"#.into(),
            stderr: String::new(),
            timed_out: false,
            spawn_failed: false,
        };
        let r = ExecutionResult::from_execution(&sample_plan(), &raw);
        assert_eq!(r.status, Status::Success);
        assert!(r.stdout_json.is_some());
        assert!(r.evidence.parsed_sdkt_result);
    }

    #[test]
    fn nonzero_exit_is_failed_even_with_valid_json() {
        // A `critical` health report still exits 1 — it must never read as
        // success just because stdout parsed.
        let raw = RawExecution {
            exit_code: Some(1),
            stdout: r#"{"health":"critical"}"#.into(),
            stderr: "Error: mismatch".into(),
            timed_out: false,
            spawn_failed: false,
        };
        let r = ExecutionResult::from_execution(&sample_plan(), &raw);
        assert_eq!(r.status, Status::Failed);
        assert_eq!(r.exit_code, Some(1));
        assert!(r.stdout_json.is_some(), "the report is still evidence");
    }

    #[test]
    fn blocked_results_never_claim_execution() {
        let e = PlanError::Blocked {
            capability_id: "deploy",
            safety: sdkt_core::registry::Safety::Mutating,
            requires_confirmation: true,
        };
        let r = ExecutionResult::refused("deploy this", &e);
        assert_eq!(r.status, Status::Blocked);
        assert!(!r.evidence.command_executed);
        assert!(r.argv.is_empty());
        assert_eq!(r.error_code.as_deref(), Some("blocked_unsafe"));
    }

    #[test]
    fn needs_clarification_for_ambiguity() {
        let e = PlanError::Parse(crate::intent::ParseError::Ambiguous {
            candidates: vec!["health"],
        });
        let r = ExecutionResult::refused("check health and verify", &e);
        assert_eq!(r.status, Status::NeedsClarification);
        assert!(!r.evidence.command_executed);
        assert_eq!(r.error_code.as_deref(), Some("ambiguous_request"));
    }

    #[test]
    fn stderr_is_never_merged_into_stdout_fields() {
        let raw = RawExecution {
            exit_code: Some(1),
            stdout: r#"{"health":"critical"}"#.into(),
            stderr: "Actionable diagnostic line".into(),
            timed_out: false,
            spawn_failed: false,
        };
        let r = ExecutionResult::from_execution(&sample_plan(), &raw);
        let json = r.to_json();
        assert!(!json.contains("Actionable diagnostic line\n\""));
        assert!(r.evidence.stderr.contains("Actionable diagnostic line"));
        let stdout_text = r.stdout_text.unwrap();
        assert!(!stdout_text.contains("Actionable diagnostic line"));
    }

    #[test]
    fn timeout_is_failed_with_no_exit_code() {
        let raw = RawExecution {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
            spawn_failed: false,
        };
        let r = ExecutionResult::from_execution(&sample_plan(), &raw);
        assert_eq!(r.status, Status::Failed);
        assert_eq!(r.exit_code, None);
        assert!(r.evidence.timed_out);
    }

    #[test]
    fn result_json_is_pure_and_versioned() {
        let raw = RawExecution {
            exit_code: Some(0),
            stdout: "{}".into(),
            stderr: String::new(),
            timed_out: false,
            spawn_failed: false,
        };
        let r = ExecutionResult::from_execution(&sample_plan(), &raw);
        let v: serde_json::Value = serde_json::from_str(&r.to_json()).expect("pure JSON");
        assert_eq!(v["schema_version"], AGENT_SCHEMA_VERSION);
        for key in [
            "request",
            "capability_id",
            "command",
            "argv",
            "status",
            "exit_code",
            "evidence",
            "explanation",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
    }
}
