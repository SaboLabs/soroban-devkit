//! Static security analysis types for Soroban contracts.
//!
//! These types are serializable so the CLI can emit them as JSON (the
//! `AuditReport` is the stable output contract for `sdkt audit`).

use serde::{Deserialize, Serialize};

/// Severity of a finding. Ordered most-to-least severe for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    #[default]
    Warning,
    Info,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Severity::Critical => "critical",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        f.write_str(s)
    }
}

/// A single audit finding produced by a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable rule identifier, e.g. `AUTH-001`.
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
    /// Optional human-readable location (function name, or `fn:binding`).
    pub location: Option<String>,
}

/// Aggregate counts by severity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AuditSummary {
    pub critical: usize,
    pub warning: usize,
    pub info: usize,
    pub total: usize,
}

/// The full result of an audit run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AuditReport {
    pub findings: Vec<Finding>,
    pub summary: AuditSummary,
}

impl AuditReport {
    /// Append a finding and update the severity counters.
    pub fn add(&mut self, f: Finding) {
        match f.severity {
            Severity::Critical => self.summary.critical += 1,
            Severity::Warning => self.summary.warning += 1,
            Severity::Info => self.summary.info += 1,
        }
        self.summary.total += 1;
        self.findings.push(f);
    }

    /// True when no findings were produced.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}
