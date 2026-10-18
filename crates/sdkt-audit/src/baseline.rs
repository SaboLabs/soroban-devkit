//! Audit baseline persistence and comparison for ratchet-style CI gating.
//!
//! Enables "no new findings" policy in CI where existing findings are tracked
//! debt and only newly introduced findings fail the build.

use crate::types::{AuditReport, AuditSummary, Finding};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Errors produced during baseline serialization, deserialization, or I/O.
#[derive(Debug, thiserror::Error)]
pub enum BaselineError {
    #[error("failed to read or write baseline file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse or serialize baseline JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// A serialized audit baseline containing metadata (sdkt version and rule set)
/// alongside the recorded findings and summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditBaseline {
    /// SDKT version that produced this baseline.
    pub version: String,
    /// Sorted list of rule IDs active during baseline generation.
    pub rules: Vec<String>,
    /// Recorded findings in the baseline.
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// Summary totals by severity.
    #[serde(default)]
    pub summary: AuditSummary,
    /// Optional nested report field to support alternative JSON baseline formats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<AuditReport>,
}

impl AuditBaseline {
    /// Create a new baseline from an audit report and active rule IDs.
    pub fn new(version: impl Into<String>, rules: Vec<String>, report: AuditReport) -> Self {
        let mut sorted_rules = rules;
        sorted_rules.sort();
        sorted_rules.dedup();
        Self {
            version: version.into(),
            rules: sorted_rules,
            findings: report.findings,
            summary: report.summary,
            report: None,
        }
    }

    /// Convert this baseline back into a standard `AuditReport`.
    pub fn to_report(&self) -> AuditReport {
        if let Some(ref r) = self.report {
            if self.findings.is_empty() {
                return r.clone();
            }
        }
        AuditReport {
            findings: self.findings.clone(),
            summary: self.summary.clone(),
        }
    }

    /// Parse a baseline from a JSON string, supporting both top-level and nested report fields.
    pub fn from_json_str(s: &str) -> Result<Self, BaselineError> {
        let mut baseline: Self = serde_json::from_str(s)?;
        if let Some(ref r) = baseline.report {
            if baseline.findings.is_empty() {
                baseline.findings = r.findings.clone();
                baseline.summary = r.summary.clone();
            }
        }
        baseline.rules.sort();
        baseline.rules.dedup();
        Ok(baseline)
    }

    /// Read and parse a baseline from a file path.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, BaselineError> {
        let content = fs::read_to_string(path)?;
        Self::from_json_str(&content)
    }

    /// Serialize the baseline to pretty-printed JSON.
    pub fn to_json_pretty(&self) -> Result<String, BaselineError> {
        let json = serde_json::to_string_pretty(self)?;
        Ok(json)
    }

    /// Save the baseline to a file.
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), BaselineError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let json = self.to_json_pretty()?;
        fs::write(path, json)?;
        Ok(())
    }
}

/// Structural key used to compare findings across audit runs:
/// `(rule_id, file, location, message)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FindingKey {
    pub rule_id: String,
    pub file: Option<String>,
    pub location: Option<String>,
    pub message: String,
}

impl FindingKey {
    pub fn from_finding(f: &Finding) -> Self {
        Self {
            rule_id: f.rule_id.clone(),
            file: f.file.clone(),
            location: f.location.clone(),
            message: f.message.clone(),
        }
    }
}

/// Result of comparing a current audit run against a saved baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineComparison {
    /// Number of findings recorded in the baseline.
    pub baseline_count: usize,
    /// Number of current findings that match known baseline findings.
    pub known_count: usize,
    /// New findings introduced since the baseline.
    pub new_findings: Vec<Finding>,
    /// Findings from the baseline that have been resolved.
    pub resolved_findings: Vec<Finding>,
    /// Number of new findings.
    pub new_count: usize,
    /// Number of resolved findings.
    pub resolved_count: usize,
    /// True when no new findings were introduced (ratchet pass).
    pub is_ratchet_pass: bool,
    /// Warning message if the rule set or sdkt version changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Compare a current audit report against a saved baseline.
///
/// Findings are keyed on `rule_id + location + message`. Multiplicities
/// (multiple identical findings) are tracked via multiset counts so resolving
/// one of two duplicate findings properly reports 1 resolved and 1 known.
pub fn compare_baseline(
    baseline: &AuditBaseline,
    current: &AuditReport,
    current_version: &str,
    current_rules: &[String],
) -> BaselineComparison {
    let mut baseline_counts: HashMap<FindingKey, usize> = HashMap::new();
    for f in &baseline.findings {
        *baseline_counts
            .entry(FindingKey::from_finding(f))
            .or_insert(0) += 1;
    }

    let mut current_counts: HashMap<FindingKey, usize> = HashMap::new();
    for f in &current.findings {
        *current_counts
            .entry(FindingKey::from_finding(f))
            .or_insert(0) += 1;
    }

    let mut matched_in_baseline: HashMap<FindingKey, usize> = HashMap::new();
    let mut new_findings = Vec::new();
    for f in &current.findings {
        let key = FindingKey::from_finding(f);
        let in_baseline = baseline_counts.get(&key).copied().unwrap_or(0);
        let matched = matched_in_baseline.entry(key).or_insert(0);
        if *matched < in_baseline {
            *matched += 1;
        } else {
            new_findings.push(f.clone());
        }
    }

    let mut matched_in_current: HashMap<FindingKey, usize> = HashMap::new();
    let mut resolved_findings = Vec::new();
    for f in &baseline.findings {
        let key = FindingKey::from_finding(f);
        let in_current = current_counts.get(&key).copied().unwrap_or(0);
        let matched = matched_in_current.entry(key).or_insert(0);
        if *matched < in_current {
            *matched += 1;
        } else {
            resolved_findings.push(f.clone());
        }
    }

    let new_count = new_findings.len();
    let resolved_count = resolved_findings.len();
    let known_count = current.findings.len().saturating_sub(new_count);
    let is_ratchet_pass = new_count == 0;

    let mut sorted_current_rules = current_rules.to_vec();
    sorted_current_rules.sort();
    sorted_current_rules.dedup();

    let mut warnings = Vec::new();
    if baseline.rules != sorted_current_rules {
        warnings.push(format!(
            "baseline was generated with a different rule set (baseline had {} rules, current has {} rules)",
            baseline.rules.len(),
            sorted_current_rules.len()
        ));
    }
    if !baseline.version.is_empty() && baseline.version != current_version {
        warnings.push(format!(
            "baseline was generated with sdkt v{} (current is v{})",
            baseline.version, current_version
        ));
    }
    let warning = if warnings.is_empty() {
        None
    } else {
        Some(warnings.join("; "))
    };

    BaselineComparison {
        baseline_count: baseline.findings.len(),
        known_count,
        new_findings,
        resolved_findings,
        new_count,
        resolved_count,
        is_ratchet_pass,
        warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Severity;
    use tempfile::tempdir;

    fn make_finding(rule: &str, loc: Option<&str>, msg: &str, sev: Severity) -> Finding {
        Finding {
            rule_id: rule.to_string(),
            severity: sev,
            message: msg.to_string(),
            file: None,
            location: loc.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_baseline_save_load_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub").join("baseline.json");

        let mut report = AuditReport::default();
        report.add(make_finding(
            "AUTH-001",
            Some("init"),
            "missing auth",
            Severity::Critical,
        ));
        report.add(make_finding(
            "MOVE-001",
            Some("transfer"),
            "use after move",
            Severity::Warning,
        ));

        let rules = vec!["MOVE-001".to_string(), "AUTH-001".to_string()];
        let baseline = AuditBaseline::new("2.7.0", rules, report.clone());

        baseline.save_to_file(&path).unwrap();
        let loaded = AuditBaseline::from_file(&path).unwrap();

        assert_eq!(loaded.version, "2.7.0");
        assert_eq!(loaded.rules, vec!["AUTH-001", "MOVE-001"]);
        assert_eq!(loaded.findings, report.findings);
        assert_eq!(loaded.summary, report.summary);
    }

    #[test]
    fn test_comparison_identical_set_passes_ratchet() {
        let mut report = AuditReport::default();
        report.add(make_finding(
            "AUTH-001",
            Some("init"),
            "missing auth",
            Severity::Critical,
        ));
        report.add(make_finding(
            "MOVE-001",
            Some("transfer"),
            "use after move",
            Severity::Warning,
        ));

        let rules = vec!["AUTH-001".to_string(), "MOVE-001".to_string()];
        let baseline = AuditBaseline::new("2.7.0", rules.clone(), report.clone());

        let diff = compare_baseline(&baseline, &report, "2.7.0", &rules);
        assert!(diff.is_ratchet_pass);
        assert_eq!(diff.new_count, 0);
        assert_eq!(diff.resolved_count, 0);
        assert_eq!(diff.baseline_count, 2);
        assert_eq!(diff.known_count, 2);
        assert!(diff.warning.is_none());
    }

    #[test]
    fn test_comparison_subset_passes_ratchet_with_resolved() {
        let mut baseline_report = AuditReport::default();
        baseline_report.add(make_finding(
            "AUTH-001",
            Some("init"),
            "missing auth",
            Severity::Critical,
        ));
        baseline_report.add(make_finding(
            "MOVE-001",
            Some("transfer"),
            "use after move",
            Severity::Warning,
        ));

        let mut current_report = AuditReport::default();
        current_report.add(make_finding(
            "AUTH-001",
            Some("init"),
            "missing auth",
            Severity::Critical,
        ));

        let rules = vec!["AUTH-001".to_string(), "MOVE-001".to_string()];
        let baseline = AuditBaseline::new("2.7.0", rules.clone(), baseline_report);

        let diff = compare_baseline(&baseline, &current_report, "2.7.0", &rules);
        assert!(diff.is_ratchet_pass);
        assert_eq!(diff.new_count, 0);
        assert_eq!(diff.resolved_count, 1);
        assert_eq!(diff.baseline_count, 2);
        assert_eq!(diff.known_count, 1);
        assert_eq!(diff.resolved_findings[0].rule_id, "MOVE-001");
    }

    #[test]
    fn test_comparison_new_finding_fails_ratchet() {
        let mut baseline_report = AuditReport::default();
        baseline_report.add(make_finding(
            "AUTH-001",
            Some("init"),
            "missing auth",
            Severity::Critical,
        ));

        let mut current_report = AuditReport::default();
        current_report.add(make_finding(
            "AUTH-001",
            Some("init"),
            "missing auth",
            Severity::Critical,
        ));
        current_report.add(make_finding(
            "AUTH-003",
            Some("admin"),
            "unprotected",
            Severity::Critical,
        ));

        let rules = vec!["AUTH-001".to_string(), "AUTH-003".to_string()];
        let baseline = AuditBaseline::new("2.7.0", rules.clone(), baseline_report);

        let diff = compare_baseline(&baseline, &current_report, "2.7.0", &rules);
        assert!(!diff.is_ratchet_pass);
        assert_eq!(diff.new_count, 1);
        assert_eq!(diff.resolved_count, 0);
        assert_eq!(diff.new_findings[0].rule_id, "AUTH-003");
        assert_eq!(diff.known_count, 1);
    }

    #[test]
    fn test_comparison_both_new_and_resolved() {
        let mut baseline_report = AuditReport::default();
        baseline_report.add(make_finding(
            "AUTH-001",
            Some("init"),
            "missing auth",
            Severity::Critical,
        ));

        let mut current_report = AuditReport::default();
        current_report.add(make_finding(
            "AUTH-002",
            Some("call"),
            "unverified call",
            Severity::Warning,
        ));

        let rules = vec!["AUTH-001".to_string(), "AUTH-002".to_string()];
        let baseline = AuditBaseline::new("2.7.0", rules.clone(), baseline_report);

        let diff = compare_baseline(&baseline, &current_report, "2.7.0", &rules);
        assert!(!diff.is_ratchet_pass);
        assert_eq!(diff.new_count, 1);
        assert_eq!(diff.resolved_count, 1);
        assert_eq!(diff.new_findings[0].rule_id, "AUTH-002");
        assert_eq!(diff.resolved_findings[0].rule_id, "AUTH-001");
        assert_eq!(diff.known_count, 0);
    }

    #[test]
    fn test_stale_rule_set_triggers_warning() {
        let report = AuditReport::default();
        let baseline_rules = vec!["AUTH-001".to_string()];
        let baseline = AuditBaseline::new("2.7.0", baseline_rules, report.clone());

        let current_rules = vec!["AUTH-001".to_string(), "AUTH-002".to_string()];
        let diff = compare_baseline(&baseline, &report, "2.7.0", &current_rules);
        assert!(diff.warning.is_some());
        assert!(diff.warning.unwrap().contains("different rule set"));
    }

    #[test]
    fn test_stale_version_triggers_warning() {
        let report = AuditReport::default();
        let rules = vec!["AUTH-001".to_string()];
        let baseline = AuditBaseline::new("1.0.0", rules.clone(), report.clone());

        let diff = compare_baseline(&baseline, &report, "2.7.0", &rules);
        assert!(diff.warning.is_some());
        assert!(diff.warning.unwrap().contains("generated with sdkt v1.0.0"));
    }

    #[test]
    fn test_comparison_differentiates_files_with_identical_findings() {
        let mut baseline_report = AuditReport::default();
        baseline_report.add(Finding {
            rule_id: "AUTH-001".to_string(),
            severity: Severity::Critical,
            message: "missing auth".to_string(),
            file: Some("contract_a.rs".to_string()),
            location: Some("init".to_string()),
        });

        let mut current_report = AuditReport::default();
        current_report.add(Finding {
            rule_id: "AUTH-001".to_string(),
            severity: Severity::Critical,
            message: "missing auth".to_string(),
            file: Some("contract_b.rs".to_string()),
            location: Some("init".to_string()),
        });

        let rules = vec!["AUTH-001".to_string()];
        let baseline = AuditBaseline::new("2.7.0", rules.clone(), baseline_report);

        let diff = compare_baseline(&baseline, &current_report, "2.7.0", &rules);
        assert!(!diff.is_ratchet_pass);
        assert_eq!(diff.new_count, 1);
        assert_eq!(diff.resolved_count, 1);
        assert_eq!(diff.new_findings[0].file.as_deref(), Some("contract_b.rs"));
        assert_eq!(
            diff.resolved_findings[0].file.as_deref(),
            Some("contract_a.rs")
        );
        assert_eq!(diff.known_count, 0);
    }
}
