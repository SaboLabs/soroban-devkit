//! Audit baselines: persistence and new-findings-only comparison.
//!
//! A *baseline* is the full [`AuditReport`] produced by a previous `sdkt audit`
//! run, persisted as JSON together with the metadata required to detect when it
//! has gone stale:
//!
//! - `sdkt_version` — the tool version that produced the baseline, and
//! - `rules` — the ids of the rules that were active at capture time.
//!
//! Comparing a fresh report against a baseline yields the findings that are
//! *new* (present now, absent from the baseline) and *resolved* (present in the
//! baseline, absent now). This is the "ratchet" gate: CI fails only when the
//! current findings are not a subset of the tracked baseline, i.e. when new
//! findings were introduced.
//!
//! No analysis logic lives here — the comparison is a mechanical diff over the
//! already-stable structured report.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AuditError;
use crate::types::{AuditReport, Finding};

/// Version of the baseline document format written by this build.
///
/// Bump when the on-disk shape changes; [`AuditBaseline::from_json`] rejects
/// documents whose `format_version` does not match.
pub const BASELINE_FORMAT_VERSION: u32 = 1;

/// The `sdkt` version embedded in newly created baselines.
///
/// This is the workspace version of `sdkt-audit`, which is kept in lockstep
/// with the `sdkt` binary via the shared workspace package version.
pub fn sdkt_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A persisted audit report plus the provenance needed to detect staleness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditBaseline {
    /// Format version of this baseline document.
    pub format_version: u32,
    /// `sdkt` version that produced the baseline.
    pub sdkt_version: String,
    /// Sorted, deduplicated ids of the rules active at capture time.
    pub rules: Vec<String>,
    /// The captured report (findings + per-severity summary).
    pub report: AuditReport,
}

impl AuditBaseline {
    /// Build a baseline from a report and the active rule ids.
    ///
    /// `rules` is sorted and deduplicated so equal rule sets always serialize
    /// identically (stable files, stable warnings).
    pub fn new(report: AuditReport, mut rules: Vec<String>) -> Self {
        rules.sort();
        rules.dedup();
        Self {
            format_version: BASELINE_FORMAT_VERSION,
            sdkt_version: sdkt_version().to_string(),
            rules,
            report,
        }
    }

    /// Serialize this baseline to pretty JSON.
    pub fn to_json(&self) -> Result<String, AuditError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse a baseline from JSON, rejecting unsupported format versions.
    pub fn from_json(json: &str) -> Result<Self, AuditError> {
        let baseline: Self = serde_json::from_str(json)?;
        baseline.validate()?;
        Ok(baseline)
    }

    /// Write this baseline to `path` as pretty JSON.
    pub fn save(&self, path: &Path) -> Result<(), AuditError> {
        std::fs::write(path, self.to_json()?)?;
        Ok(())
    }

    /// Load a baseline from `path`.
    pub fn load(path: &Path) -> Result<Self, AuditError> {
        let raw = std::fs::read_to_string(path)?;
        Self::from_json(&raw)
    }

    /// Number of findings captured in the baseline.
    pub fn known_findings(&self) -> usize {
        self.report.findings.len()
    }

    fn validate(&self) -> Result<(), AuditError> {
        if self.format_version != BASELINE_FORMAT_VERSION {
            return Err(AuditError::BaselineFormat {
                found: self.format_version,
                expected: BASELINE_FORMAT_VERSION,
            });
        }
        Ok(())
    }
}

/// Outcome of comparing a current report against a baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineComparison {
    /// `sdkt` version that produced the baseline.
    pub sdkt_version: String,
    /// Rule ids recorded in the baseline.
    pub baseline_rules: Vec<String>,
    /// Findings recorded in the baseline.
    pub known_findings: usize,
    /// Findings in the current report.
    pub total_findings: usize,
    /// Findings present now but not in the baseline.
    pub new_findings: Vec<Finding>,
    /// Findings present in the baseline but no longer reported.
    pub resolved_findings: Vec<Finding>,
    /// `new_findings.len()` (serialized for convenience).
    pub new_count: usize,
    /// `resolved_findings.len()` (serialized for convenience).
    pub resolved_count: usize,
    /// Non-fatal staleness warnings (version / rule-set drift).
    pub warnings: Vec<String>,
    /// True when no new findings were introduced — the ratchet pass case.
    pub passed: bool,
}

/// Identity of a finding for baseline diffing.
///
/// Keyed on `rule_id` + `location` + `message`, exactly as specified by the
/// issue (severity is intentionally excluded so a severity reclassification
/// does not masquerade as a new or resolved finding).
fn finding_key(f: &Finding) -> (String, Option<String>, String) {
    (f.rule_id.clone(), f.location.clone(), f.message.clone())
}

/// Compare `report` against `baseline`.
///
/// `current_rules` is the rule set active for this run and is used only to
/// produce staleness warnings; it does not affect the finding diff.
pub fn compare_to_baseline(
    report: &AuditReport,
    baseline: &AuditBaseline,
    current_rules: &[String],
) -> BaselineComparison {
    let baseline_keys: HashSet<(String, Option<String>, String)> =
        baseline.report.findings.iter().map(finding_key).collect();
    let current_keys: HashSet<(String, Option<String>, String)> =
        report.findings.iter().map(finding_key).collect();

    let new_findings: Vec<Finding> = report
        .findings
        .iter()
        .filter(|f| !baseline_keys.contains(&finding_key(f)))
        .cloned()
        .collect();
    let resolved_findings: Vec<Finding> = baseline
        .report
        .findings
        .iter()
        .filter(|f| !current_keys.contains(&finding_key(f)))
        .cloned()
        .collect();

    let passed = new_findings.is_empty();

    BaselineComparison {
        sdkt_version: baseline.sdkt_version.clone(),
        baseline_rules: baseline.rules.clone(),
        known_findings: baseline.report.findings.len(),
        total_findings: report.findings.len(),
        new_count: new_findings.len(),
        resolved_count: resolved_findings.len(),
        new_findings,
        resolved_findings,
        warnings: stale_warnings(baseline, current_rules),
        passed,
    }
}

/// Non-fatal warnings describing how `baseline` is stale relative to the
/// current tool version and rule set. Empty when the baseline is current.
pub fn stale_warnings(baseline: &AuditBaseline, current_rules: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();

    if baseline.sdkt_version != sdkt_version() {
        warnings.push(format!(
            "baseline was created with sdkt {} (current {})",
            baseline.sdkt_version,
            sdkt_version()
        ));
    }

    let mut current = current_rules.to_vec();
    current.sort();
    current.dedup();

    let mut known = baseline.rules.clone();
    known.sort();
    known.dedup();

    if known != current {
        let added: Vec<&str> = current
            .iter()
            .filter(|r| !known.contains(*r))
            .map(String::as_str)
            .collect();
        let removed: Vec<&str> = known
            .iter()
            .filter(|r| !current.contains(*r))
            .map(String::as_str)
            .collect();

        let mut parts = Vec::new();
        if !added.is_empty() {
            parts.push(format!("new rules: {}", added.join(", ")));
        }
        if !removed.is_empty() {
            parts.push(format!("removed rules: {}", removed.join(", ")));
        }
        warnings.push(format!("baseline rule set differs ({})", parts.join("; ")));
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AuditSummary, Severity};

    fn finding(rule: &str, location: Option<&str>, message: &str) -> Finding {
        Finding {
            rule_id: rule.to_string(),
            severity: Severity::Critical,
            message: message.to_string(),
            location: location.map(str::to_string),
        }
    }

    fn report(findings: Vec<Finding>) -> AuditReport {
        let mut report = AuditReport::default();
        for f in findings {
            report.add(f);
        }
        report
    }

    fn rules() -> Vec<String> {
        vec!["AUTH-001".to_string(), "MOVE-001".to_string()]
    }

    #[test]
    fn baseline_round_trips_through_json() {
        let report = report(vec![finding("AUTH-001", Some("mint"), "missing auth")]);
        let baseline = AuditBaseline::new(report.clone(), rules());

        let json = baseline.to_json().unwrap();
        let back = AuditBaseline::from_json(&json).unwrap();

        assert_eq!(baseline, back);
        assert_eq!(back.report, report);
        assert_eq!(back.report.summary, report.summary);
        assert_eq!(back.format_version, BASELINE_FORMAT_VERSION);
        assert_eq!(back.sdkt_version, sdkt_version());
        assert_eq!(
            back.rules,
            vec!["AUTH-001".to_string(), "MOVE-001".to_string()]
        );
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit-baseline.json");

        let report = report(vec![finding("AUTH-003", None, "unguarded initialize")]);
        let baseline = AuditBaseline::new(report.clone(), rules());
        baseline.save(&path).unwrap();

        let back = AuditBaseline::load(&path).unwrap();
        assert_eq!(back.report, report);
        assert_eq!(back.known_findings(), 1);
    }

    #[test]
    fn new_finding_is_detected() {
        let baseline = AuditBaseline::new(
            report(vec![finding("AUTH-001", Some("mint"), "m1")]),
            rules(),
        );
        let current = report(vec![
            finding("AUTH-001", Some("mint"), "m1"),
            finding("MOVE-001", Some("transfer"), "m2"),
        ]);

        let cmp = compare_to_baseline(&current, &baseline, &rules());
        assert_eq!(cmp.new_count, 1);
        assert_eq!(cmp.resolved_count, 0);
        assert_eq!(cmp.new_findings[0].rule_id, "MOVE-001");
        assert!(!cmp.passed);
        assert!(cmp.warnings.is_empty());
    }

    #[test]
    fn resolved_finding_is_detected() {
        let baseline = AuditBaseline::new(
            report(vec![
                finding("AUTH-001", Some("mint"), "m1"),
                finding("MOVE-001", Some("transfer"), "m2"),
            ]),
            rules(),
        );
        let current = report(vec![finding("AUTH-001", Some("mint"), "m1")]);

        let cmp = compare_to_baseline(&current, &baseline, &rules());
        assert_eq!(cmp.new_count, 0);
        assert_eq!(cmp.resolved_count, 1);
        assert_eq!(cmp.resolved_findings[0].rule_id, "MOVE-001");
        assert!(cmp.passed);
    }

    #[test]
    fn identical_findings_pass() {
        let findings = vec![
            finding("AUTH-001", Some("mint"), "m1"),
            finding("AUTH-003", Some("initialize"), "m2"),
        ];
        let baseline = AuditBaseline::new(report(findings.clone()), rules());
        let cmp = compare_to_baseline(&report(findings), &baseline, &rules());

        assert_eq!(cmp.new_count, 0);
        assert_eq!(cmp.resolved_count, 0);
        assert!(cmp.passed);
    }

    #[test]
    fn subset_is_a_ratchet_pass() {
        let baseline = AuditBaseline::new(
            report(vec![
                finding("AUTH-001", Some("mint"), "m1"),
                finding("AUTH-003", Some("initialize"), "m2"),
                finding("MOVE-001", Some("transfer"), "m3"),
            ]),
            rules(),
        );
        // Only one of the three tracked findings remains — still a pass,
        // because no *new* findings were introduced.
        let current = report(vec![finding("AUTH-003", Some("initialize"), "m2")]);

        let cmp = compare_to_baseline(&current, &baseline, &rules());
        assert!(cmp.passed);
        assert_eq!(cmp.new_count, 0);
        assert_eq!(cmp.resolved_count, 2);
    }

    #[test]
    fn severity_change_is_not_a_new_finding() {
        let baseline = AuditBaseline::new(
            report(vec![finding("AUTH-001", Some("mint"), "m1")]),
            rules(),
        );
        let mut changed = finding("AUTH-001", Some("mint"), "m1");
        changed.severity = Severity::Info;

        let cmp = compare_to_baseline(&report(vec![changed]), &baseline, &rules());
        assert_eq!(cmp.new_count, 0);
        assert_eq!(cmp.resolved_count, 0);
        assert!(cmp.passed);
    }

    #[test]
    fn stale_rule_set_warns() {
        let baseline = AuditBaseline::new(AuditReport::default(), rules());
        let warnings = stale_warnings(&baseline, &["AUTH-001".to_string()]);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("rule set differs"));
        assert!(warnings[0].contains("removed rules: MOVE-001"));
    }

    #[test]
    fn stale_version_warns() {
        let mut baseline = AuditBaseline::new(AuditReport::default(), rules());
        baseline.sdkt_version = "0.0.0-old".to_string();

        let warnings = stale_warnings(&baseline, &rules());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("0.0.0-old"));
        assert!(warnings[0].contains(sdkt_version()));
    }

    #[test]
    fn current_baseline_has_no_warnings() {
        let baseline = AuditBaseline::new(AuditReport::default(), rules());
        assert!(stale_warnings(&baseline, &rules()).is_empty());
    }

    #[test]
    fn unsupported_format_version_is_rejected() {
        let mut baseline = AuditBaseline::new(AuditReport::default(), rules());
        baseline.format_version = BASELINE_FORMAT_VERSION + 1;
        let json = serde_json::to_string(&baseline).unwrap();

        let err = AuditBaseline::from_json(&json).unwrap_err();
        assert!(matches!(err, AuditError::BaselineFormat { .. }));
    }

    #[test]
    fn summary_is_preserved_in_baseline() {
        let report = report(vec![
            finding("AUTH-001", None, "a"),
            finding("MOVE-001", None, "b"),
        ]);
        let baseline = AuditBaseline::new(report.clone(), rules());
        assert_eq!(
            baseline.report.summary,
            AuditSummary {
                critical: 2,
                warning: 0,
                info: 0,
                total: 2,
            }
        );
    }
}
