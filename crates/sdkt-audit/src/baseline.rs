//! Baseline report persistence and comparison for `sdkt audit`.
//!
//! Enables ratchet-style CI gating: save a known-debt snapshot with
//! `--save-baseline <path>` and gate subsequent runs on *new* findings only
//! with `--baseline <path>`.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{AuditReport, AuditSummary, Finding};

/// Errors produced when loading or saving an audit baseline file.
#[derive(Debug, Error)]
pub enum BaselineError {
    /// I/O error reading or writing the baseline file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization or deserialization failure.
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Persisted audit baseline report.
///
/// Wraps the standard [`AuditReport`] fields (`findings` and `summary`) with
/// metadata (`sdkt_version` and `rules`) so stale baselines can be detected
/// when the tool version or active rule set changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineReport {
    /// Version of `sdkt` that produced this baseline.
    pub sdkt_version: String,
    /// Active rule IDs that ran when this baseline was produced.
    pub rules: Vec<String>,
    /// Findings recorded in the baseline.
    pub findings: Vec<Finding>,
    /// Aggregate severity counts for the recorded findings.
    pub summary: AuditSummary,
}

/// Alias for [`BaselineReport`].
pub type AuditBaseline = BaselineReport;

/// Composite key identifying a finding across audit runs:
/// `(file, rule_id, location, message)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FindingKey {
    pub file: Option<String>,
    pub rule_id: String,
    pub location: Option<String>,
    pub message: String,
}

impl FindingKey {
    /// Extract the comparison key from a [`Finding`].
    pub fn from_finding(finding: &Finding) -> Self {
        Self {
            file: finding.file.clone(),
            rule_id: finding.rule_id.clone(),
            location: finding.location.clone(),
            message: finding.message.clone(),
        }
    }
}

/// Result of comparing a current [`AuditReport`] against a [`BaselineReport`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineComparison {
    /// Total number of findings recorded in the baseline.
    pub baseline_total: usize,
    /// Findings present in the current run that were not in the baseline.
    pub new_findings: Vec<Finding>,
    /// Findings present in the baseline that are no longer in the current run.
    pub resolved_findings: Vec<Finding>,
    /// Findings present in both the baseline and the current run.
    pub unchanged_findings: Vec<Finding>,
    /// Aggregate severity summary of the current audit report.
    pub summary: AuditSummary,
    /// Staleness warnings (e.g. when the rule set or `sdkt` version changed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl BaselineComparison {
    /// Returns `true` when at least one new finding was introduced relative to
    /// the baseline.
    pub fn has_new_findings(&self) -> bool {
        !self.new_findings.is_empty()
    }

    /// Compute an [`AuditSummary`] over `new_findings` only.
    pub fn new_findings_summary(&self) -> AuditSummary {
        let mut report = AuditReport::default();
        for f in &self.new_findings {
            report.add(f.clone());
        }
        report.summary
    }
}

impl BaselineReport {
    /// Construct a [`BaselineReport`] from an [`AuditReport`], version string,
    /// and list of active rule IDs.
    pub fn from_report(report: &AuditReport, sdkt_version: &str, rules: &[String]) -> Self {
        Self {
            sdkt_version: sdkt_version.to_string(),
            rules: rules.to_vec(),
            findings: report.findings.clone(),
            summary: report.summary.clone(),
        }
    }

    /// Convert the baseline back into an [`AuditReport`].
    pub fn to_report(&self) -> AuditReport {
        AuditReport {
            findings: self.findings.clone(),
            summary: self.summary.clone(),
        }
    }

    /// Persist this baseline as pretty-printed JSON at `path`.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), BaselineError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a [`BaselineReport`] from a JSON file at `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, BaselineError> {
        let raw = std::fs::read_to_string(path)?;
        let baseline = serde_json::from_str(&raw)?;
        Ok(baseline)
    }

    /// Compare `current` findings against this baseline, keyed on
    /// `(file, rule_id, location, message)` with multiplicity preserved.
    pub fn compare(&self, current: &AuditReport) -> BaselineComparison {
        let mut baseline_counts: HashMap<FindingKey, usize> = HashMap::new();
        for f in &self.findings {
            *baseline_counts.entry(FindingKey::from_finding(f)).or_insert(0) += 1;
        }

        let mut current_counts: HashMap<FindingKey, usize> = HashMap::new();
        for f in &current.findings {
            *current_counts.entry(FindingKey::from_finding(f)).or_insert(0) += 1;
        }

        let mut new_findings = Vec::new();
        let mut unchanged_findings = Vec::new();
        for f in &current.findings {
            let key = FindingKey::from_finding(f);
            if let Some(count) = baseline_counts.get_mut(&key) {
                if *count > 0 {
                    *count -= 1;
                    unchanged_findings.push(f.clone());
                    continue;
                }
            }
            new_findings.push(f.clone());
        }

        let mut resolved_findings = Vec::new();
        for f in &self.findings {
            let key = FindingKey::from_finding(f);
            if let Some(count) = current_counts.get_mut(&key) {
                if *count > 0 {
                    *count -= 1;
                    continue;
                }
            }
            resolved_findings.push(f.clone());
        }

        BaselineComparison {
            baseline_total: self.findings.len(),
            new_findings,
            resolved_findings,
            unchanged_findings,
            summary: current.summary.clone(),
            warnings: Vec::new(),
        }
    }

    /// Check whether this baseline was produced by a different `sdkt` version
    /// or rule set than the current run.
    ///
    /// Returns a list of human-readable warning messages (empty when the
    /// version and rule set match).
    pub fn check_staleness(&self, current_version: &str, current_rules: &[String]) -> Vec<String> {
        let mut warnings = Vec::new();

        if self.sdkt_version != current_version {
            warnings.push(format!(
                "stale baseline: baseline was created with sdkt version '{}' (current version is '{}')",
                self.sdkt_version, current_version
            ));
        }

        let baseline_set: BTreeSet<&str> = self.rules.iter().map(String::as_str).collect();
        let current_set: BTreeSet<&str> = current_rules.iter().map(String::as_str).collect();

        if baseline_set != current_set {
            let added: Vec<&str> = current_set.difference(&baseline_set).copied().collect();
            let removed: Vec<&str> = baseline_set.difference(&current_set).copied().collect();
            let mut details = Vec::new();
            if !added.is_empty() {
                details.push(format!("added: {}", added.join(", ")));
            }
            if !removed.is_empty() {
                details.push(format!("removed: {}", removed.join(", ")));
            }
            warnings.push(format!(
                "stale baseline: rule set changed ({})",
                details.join("; ")
            ));
        }

        warnings
    }
}

/// Build and save a [`BaselineReport`] to `path`.
pub fn save_baseline(
    path: impl AsRef<Path>,
    report: &AuditReport,
    sdkt_version: &str,
    rules: &[String],
) -> Result<BaselineReport, BaselineError> {
    let baseline = BaselineReport::from_report(report, sdkt_version, rules);
    baseline.save(path)?;
    Ok(baseline)
}

/// Load a [`BaselineReport`] from `path`.
pub fn load_baseline(path: impl AsRef<Path>) -> Result<BaselineReport, BaselineError> {
    BaselineReport::load(path)
}

/// Compare `current` findings against `baseline`.
pub fn compare_with_baseline(
    baseline: &BaselineReport,
    current: &AuditReport,
) -> BaselineComparison {
    baseline.compare(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Severity;
    use tempfile::TempDir;

    fn sample_finding(rule_id: &str, severity: Severity, location: &str, message: &str) -> Finding {
        Finding {
            rule_id: rule_id.to_string(),
            severity,
            message: message.to_string(),
            file: None,
            location: Some(location.to_string()),
        }
    }

    fn default_rules() -> Vec<String> {
        vec![
            "AUTH-001".to_string(),
            "AUTH-002".to_string(),
            "AUTH-003".to_string(),
            "AUTH-004".to_string(),
            "MOVE-001".to_string(),
            "MATH-001".to_string(),
            "CEI-001".to_string(),
        ]
    }

    #[test]
    fn baseline_save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let baseline_path = dir.path().join("audit-baseline.json");

        let mut report = AuditReport::default();
        report.add(sample_finding(
            "AUTH-001",
            Severity::Critical,
            "mint",
            "Function `mint` looks privileged but does not call require_auth()",
        ));
        report.add(sample_finding(
            "MATH-001",
            Severity::Warning,
            "quote",
            "Function `quote` divides before multiplying; division may truncate precision",
        ));

        let rules = default_rules();
        let saved = save_baseline(&baseline_path, &report, "2.5.0", &rules).unwrap();
        let loaded = load_baseline(&baseline_path).unwrap();

        assert_eq!(saved, loaded);
        assert_eq!(loaded.sdkt_version, "2.5.0");
        assert_eq!(loaded.rules, rules);
        assert_eq!(loaded.findings.len(), 2);
        assert_eq!(loaded.summary.critical, 1);
        assert_eq!(loaded.summary.warning, 1);
        assert_eq!(loaded.summary.total, 2);
        assert_eq!(loaded.to_report(), report);
    }

    #[test]
    fn comparison_detects_new_finding() {
        let f1 = sample_finding("AUTH-001", Severity::Critical, "mint", "missing auth");
        let f2 = sample_finding(
            "AUTH-003",
            Severity::Critical,
            "initialize_admin",
            "missing require_auth",
        );

        let mut base_report = AuditReport::default();
        base_report.add(f1.clone());
        let baseline = BaselineReport::from_report(&base_report, "2.5.0", &default_rules());

        let mut current_report = AuditReport::default();
        current_report.add(f1.clone());
        current_report.add(f2.clone());

        let cmp = baseline.compare(&current_report);
        assert!(cmp.has_new_findings());
        assert_eq!(cmp.baseline_total, 1);
        assert_eq!(cmp.new_findings, vec![f2]);
        assert!(cmp.resolved_findings.is_empty());
        assert_eq!(cmp.unchanged_findings, vec![f1]);
    }

    #[test]
    fn comparison_detects_resolved_finding() {
        let f1 = sample_finding("AUTH-001", Severity::Critical, "mint", "missing auth");
        let f2 = sample_finding("MATH-001", Severity::Warning, "quote", "div before mul");

        let mut base_report = AuditReport::default();
        base_report.add(f1.clone());
        base_report.add(f2.clone());
        let baseline = BaselineReport::from_report(&base_report, "2.5.0", &default_rules());

        let mut current_report = AuditReport::default();
        current_report.add(f1.clone());

        let cmp = baseline.compare(&current_report);
        assert!(!cmp.has_new_findings());
        assert_eq!(cmp.baseline_total, 2);
        assert!(cmp.new_findings.is_empty());
        assert_eq!(cmp.resolved_findings, vec![f2]);
        assert_eq!(cmp.unchanged_findings, vec![f1]);
    }

    #[test]
    fn comparison_identical_set_has_no_new_findings() {
        let f1 = sample_finding("AUTH-001", Severity::Critical, "mint", "missing auth");
        let f2 = sample_finding("MATH-001", Severity::Warning, "quote", "div before mul");

        let mut report = AuditReport::default();
        report.add(f1.clone());
        report.add(f2.clone());
        let baseline = BaselineReport::from_report(&report, "2.5.0", &default_rules());

        let cmp = baseline.compare(&report);
        assert!(!cmp.has_new_findings());
        assert_eq!(cmp.baseline_total, 2);
        assert!(cmp.new_findings.is_empty());
        assert!(cmp.resolved_findings.is_empty());
        assert_eq!(cmp.unchanged_findings.len(), 2);
    }

    #[test]
    fn comparison_subset_ratchet_passes() {
        let f1 = sample_finding("AUTH-001", Severity::Critical, "mint", "missing auth");
        let f2 = sample_finding("MATH-001", Severity::Warning, "quote", "div before mul");
        let f3 = sample_finding("CEI-001", Severity::Warning, "swap", "external call first");

        let mut base_report = AuditReport::default();
        base_report.add(f1.clone());
        base_report.add(f2.clone());
        base_report.add(f3.clone());
        let baseline = BaselineReport::from_report(&base_report, "2.5.0", &default_rules());

        // Refactoring fixed f2 and f3, leaving only f1 (strict subset)
        let mut current_report = AuditReport::default();
        current_report.add(f1.clone());

        let cmp = baseline.compare(&current_report);
        assert!(!cmp.has_new_findings());
        assert_eq!(cmp.baseline_total, 3);
        assert!(cmp.new_findings.is_empty());
        assert_eq!(cmp.resolved_findings, vec![f2, f3]);
        assert_eq!(cmp.unchanged_findings, vec![f1]);
    }

    #[test]
    fn staleness_check_warns_on_rule_set_or_version_change() {
        let report = AuditReport::default();
        let rules = default_rules();
        let baseline = BaselineReport::from_report(&report, "2.4.0", &rules);

        // Matching version and rules -> no warnings
        assert!(baseline.check_staleness("2.4.0", &rules).is_empty());

        // Different version -> warning
        let version_warnings = baseline.check_staleness("2.5.0", &rules);
        assert_eq!(version_warnings.len(), 1);
        assert!(version_warnings[0].contains("version"));

        // Different rule set -> warning
        let fewer_rules = vec!["AUTH-001".to_string(), "AUTH-002".to_string()];
        let rule_warnings = baseline.check_staleness("2.4.0", &fewer_rules);
        assert_eq!(rule_warnings.len(), 1);
        assert!(rule_warnings[0].contains("rule set changed"));
    }

    #[test]
    fn comparison_identical_findings_in_different_source_files() {
        let f_a = Finding {
            rule_id: "AUTH-001".to_string(),
            severity: Severity::Critical,
            message: "missing auth".to_string(),
            file: Some("contracts/a.rs".to_string()),
            location: Some("mint".to_string()),
        };
        let f_b = Finding {
            rule_id: "AUTH-001".to_string(),
            severity: Severity::Critical,
            message: "missing auth".to_string(),
            file: Some("contracts/b.rs".to_string()),
            location: Some("mint".to_string()),
        };

        let mut base_report = AuditReport::default();
        base_report.add(f_a.clone());
        let baseline = BaselineReport::from_report(&base_report, "2.5.0", &default_rules());

        let mut current_report = AuditReport::default();
        current_report.add(f_a.clone());
        current_report.add(f_b.clone());

        let cmp = baseline.compare(&current_report);
        assert!(cmp.has_new_findings());
        assert_eq!(cmp.baseline_total, 1);
        assert_eq!(cmp.new_findings, vec![f_b.clone()]);
        assert_eq!(cmp.unchanged_findings, vec![f_a.clone()]);
        assert!(cmp.resolved_findings.is_empty());

        // When b replaces a, a is resolved and b is new
        let mut replaced_report = AuditReport::default();
        replaced_report.add(f_b.clone());
        let cmp_replaced = baseline.compare(&replaced_report);
        assert!(cmp_replaced.has_new_findings());
        assert_eq!(cmp_replaced.new_findings, vec![f_b]);
        assert_eq!(cmp_replaced.resolved_findings, vec![f_a]);
        assert!(cmp_replaced.unchanged_findings.is_empty());
    }

    #[test]
    fn comparison_duplicate_identical_findings_where_current_contains_more_than_baseline() {
        let f = sample_finding("AUTH-001", Severity::Critical, "mint", "missing auth");

        let mut base_report = AuditReport::default();
        base_report.add(f.clone());
        let baseline = BaselineReport::from_report(&base_report, "2.5.0", &default_rules());

        // Current report has 2 occurrences of the same finding
        let mut current_report = AuditReport::default();
        current_report.add(f.clone());
        current_report.add(f.clone());

        let cmp = baseline.compare(&current_report);
        assert!(cmp.has_new_findings());
        assert_eq!(cmp.baseline_total, 1);
        assert_eq!(cmp.new_findings, vec![f.clone()]);
        assert_eq!(cmp.unchanged_findings, vec![f.clone()]);
        assert!(cmp.resolved_findings.is_empty());
    }

    #[test]
    fn comparison_resolved_findings_with_multiplicity_and_identity() {
        let f_a = Finding {
            rule_id: "AUTH-001".to_string(),
            severity: Severity::Critical,
            message: "missing auth".to_string(),
            file: Some("contracts/a.rs".to_string()),
            location: Some("mint".to_string()),
        };
        let f_b = Finding {
            rule_id: "AUTH-001".to_string(),
            severity: Severity::Critical,
            message: "missing auth".to_string(),
            file: Some("contracts/b.rs".to_string()),
            location: Some("mint".to_string()),
        };

        // Baseline has 2 occurrences in a.rs and 1 in b.rs
        let mut base_report = AuditReport::default();
        base_report.add(f_a.clone());
        base_report.add(f_a.clone());
        base_report.add(f_b.clone());
        let baseline = BaselineReport::from_report(&base_report, "2.5.0", &default_rules());

        // Current report has only 1 occurrence in a.rs and 0 in b.rs
        let mut current_report = AuditReport::default();
        current_report.add(f_a.clone());

        let cmp = baseline.compare(&current_report);
        assert!(!cmp.has_new_findings());
        assert_eq!(cmp.baseline_total, 3);
        assert!(cmp.new_findings.is_empty());
        assert_eq!(cmp.unchanged_findings, vec![f_a.clone()]);
        assert_eq!(cmp.resolved_findings, vec![f_a.clone(), f_b.clone()]);
    }
}
