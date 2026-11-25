//! Compare two fuzz-campaign evidence files (`sdkt fuzz compare`).
//!
//! Pure and read-only: given a baseline [`CampaignEvidence`] and a new one,
//! classify every finding as NEW (only in the new run), FIXED (only in the
//! baseline), or PERSISTING (in both). Findings are matched by
//! [`FindingEvidence::artifact_hash`] only — no fuzzy matching on reason
//! code or function name.
//!
//! The two campaigns must target the same artifact: a different
//! `config.wasm_sha256` is an error, never a comparison of unrelated
//! campaigns. Differences in seed or case count are informational only.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::evidence::{CampaignEvidence, FindingEvidence};

/// Result of comparing two campaign evidence files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceComparison {
    /// `wasm_sha256` of the baseline campaign's artifact.
    pub baseline_wasm_sha256: String,
    /// `wasm_sha256` of the new campaign's artifact (equal to the baseline's).
    pub new_wasm_sha256: String,
    /// Findings present in the new run but not in the baseline.
    pub new: Vec<FindingEvidence>,
    /// Findings present in the baseline but not in the new run.
    pub fixed: Vec<FindingEvidence>,
    /// Findings present in both runs.
    pub persisting: Vec<FindingEvidence>,
}

impl EvidenceComparison {
    /// True when the new run introduced at least one finding.
    pub fn has_new_findings(&self) -> bool {
        !self.new.is_empty()
    }

    /// `(new, fixed, persisting)` counts.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.new.len(), self.fixed.len(), self.persisting.len())
    }
}

/// Compare two campaign evidence snapshots.
///
/// Returns `Err` naming both hashes when the campaigns target different
/// WASM artifacts. Each list is sorted by `(reason_code, function, case_id)`
/// so reports are deterministic.
pub fn compare_evidence(
    baseline: &CampaignEvidence,
    new: &CampaignEvidence,
) -> Result<EvidenceComparison, String> {
    if baseline.config.wasm_sha256 != new.config.wasm_sha256 {
        return Err(format!(
            "campaigns target different WASM artifacts: baseline wasm_sha256={} new wasm_sha256={}",
            baseline.config.wasm_sha256, new.config.wasm_sha256
        ));
    }

    let baseline_hashes: HashSet<&str> = baseline
        .findings
        .iter()
        .map(|f| f.artifact_hash.as_str())
        .collect();
    let new_hashes: HashSet<&str> = new
        .findings
        .iter()
        .map(|f| f.artifact_hash.as_str())
        .collect();

    let mut new_findings: Vec<FindingEvidence> = new
        .findings
        .iter()
        .filter(|f| !baseline_hashes.contains(f.artifact_hash.as_str()))
        .cloned()
        .collect();
    let mut fixed: Vec<FindingEvidence> = baseline
        .findings
        .iter()
        .filter(|f| !new_hashes.contains(f.artifact_hash.as_str()))
        .cloned()
        .collect();
    let mut persisting: Vec<FindingEvidence> = new
        .findings
        .iter()
        .filter(|f| baseline_hashes.contains(f.artifact_hash.as_str()))
        .cloned()
        .collect();

    // Deterministic grouping: reason_code, then function, then case_id.
    let sort_key =
        |f: &FindingEvidence| (f.reason_code.clone(), f.function.clone(), f.case_id.clone());
    new_findings.sort_by_key(sort_key);
    fixed.sort_by_key(sort_key);
    persisting.sort_by_key(sort_key);

    Ok(EvidenceComparison {
        baseline_wasm_sha256: baseline.config.wasm_sha256.clone(),
        new_wasm_sha256: new.config.wasm_sha256.clone(),
        new: new_findings,
        fixed,
        persisting,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::CampaignConfig;
    use std::collections::BTreeMap;

    fn finding(case_id: &str, function: &str, reason_code: &str, hash: &str) -> FindingEvidence {
        FindingEvidence {
            case_id: case_id.to_string(),
            function: function.to_string(),
            reason_code: reason_code.to_string(),
            artifact_hash: hash.to_string(),
            minimized: false,
        }
    }

    fn evidence(wasm_sha256: &str, findings: Vec<FindingEvidence>) -> CampaignEvidence {
        CampaignEvidence {
            campaign_completed: true,
            config: CampaignConfig {
                wasm_sha256: wasm_sha256.to_string(),
                ..CampaignConfig::default()
            },
            case_count: findings.len(),
            passed: 0,
            expected_errors: 0,
            findings,
            minimized_findings: 0,
            artifacts_available: false,
            replay: BTreeMap::new(),
            deterministic: true,
            schema_version: crate::CAMPAIGN_EVIDENCE_SCHEMA_VERSION,
            tool: "sdkt".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    #[test]
    fn three_way_classification() {
        let baseline = evidence(
            "aa",
            vec![
                finding("case0001", "increment", "RETURN_MISMATCH", "h1"),
                finding("case0002", "increment", "UNEXPECTED_ERROR", "h2"),
            ],
        );
        let new = evidence(
            "aa",
            vec![
                finding("case0001", "increment", "RETURN_MISMATCH", "h1"),
                finding("case0003", "decrement", "RETURN_MISMATCH", "h3"),
            ],
        );
        let cmp = compare_evidence(&baseline, &new).unwrap();
        assert_eq!(cmp.counts(), (1, 1, 1));
        assert_eq!(cmp.new[0].artifact_hash, "h3");
        assert_eq!(cmp.fixed[0].artifact_hash, "h2");
        assert_eq!(cmp.persisting[0].artifact_hash, "h1");
        assert!(cmp.has_new_findings());
    }

    #[test]
    fn empty_baseline_means_everything_is_new() {
        let baseline = evidence("aa", vec![]);
        let new = evidence(
            "aa",
            vec![finding("case0001", "f", "RETURN_MISMATCH", "h1")],
        );
        let cmp = compare_evidence(&baseline, &new).unwrap();
        assert_eq!(cmp.counts(), (1, 0, 0));
        assert!(cmp.has_new_findings());
    }

    #[test]
    fn empty_new_means_everything_is_fixed() {
        let baseline = evidence(
            "aa",
            vec![finding("case0001", "f", "RETURN_MISMATCH", "h1")],
        );
        let new = evidence("aa", vec![]);
        let cmp = compare_evidence(&baseline, &new).unwrap();
        assert_eq!(cmp.counts(), (0, 1, 0));
        assert!(!cmp.has_new_findings());
    }

    #[test]
    fn all_fixed_exits_zero_case() {
        // Same pair with the new finding removed: 0 new, exits 0.
        let baseline = evidence(
            "aa",
            vec![finding("case0001", "f", "RETURN_MISMATCH", "h1")],
        );
        let new = evidence("aa", vec![]);
        let cmp = compare_evidence(&baseline, &new).unwrap();
        assert_eq!(cmp.new.len(), 0);
        assert!(!cmp.has_new_findings());
    }

    #[test]
    fn wasm_mismatch_is_an_error_naming_both_hashes() {
        let baseline = evidence("aa", vec![]);
        let new = evidence("bb", vec![]);
        let err = compare_evidence(&baseline, &new).unwrap_err();
        assert!(err.contains("aa"), "{err}");
        assert!(err.contains("bb"), "{err}");
    }

    #[test]
    fn matched_by_hash_only_not_by_reason_or_function() {
        // Same reason_code + function, different hash: still NEW, not persisting.
        let baseline = evidence(
            "aa",
            vec![finding("case0001", "f", "RETURN_MISMATCH", "h1")],
        );
        let new = evidence(
            "aa",
            vec![finding("case0001", "f", "RETURN_MISMATCH", "h2")],
        );
        let cmp = compare_evidence(&baseline, &new).unwrap();
        assert_eq!(cmp.counts(), (1, 1, 0));
    }
}
