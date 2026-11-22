//! Release-assurance evidence contract (Phase 3).
//!
//! This module is the *only* bridge between a completed fuzz campaign and
//! release assurance. It is deliberately passive: it never runs a campaign,
//! never executes a case, and never touches the Soroban host. It records
//! what a campaign produced so a release-assurance consumer can decide what
//! to do with it.
//!
//! Only [`crate::Classification::Finding`] becomes security evidence. PASS
//! and EXPECTED_ERROR are recorded as counts, never as findings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::artifact::FindingArtifact;
use crate::campaign::CampaignResult;
use crate::replay::ReplayOutcome;

/// Replay status for one artifact, or the campaign as a whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ReplayStatus {
    /// No artifact was replayed (replay is opt-in).
    NotReplayed,
    /// The artifact replayed and reproduced the recorded finding.
    Reproduced { reason_code: String },
    /// The artifact replayed but disagreed with the recorded finding.
    Mismatch { reason_code: String },
    /// Replay could not be carried out (invalid input, wrong WASM, …).
    Error { reason: String },
}

/// Evidence a completed campaign hands to release assurance.
///
/// Every field is a value derived from the campaign itself. Nothing here is
/// a claim about completeness: `findings` is what the campaign produced
/// under its declared rules, not a guarantee about the contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampaignEvidence {
    /// True when the campaign ran to completion and executed at least one case.
    pub campaign_completed: bool,
    /// Campaign configuration snapshot (deterministic inputs).
    pub config: crate::artifact::CampaignConfig,
    /// Cases executed.
    pub case_count: usize,
    /// Cases classified PASS.
    pub passed: usize,
    /// Cases classified EXPECTED_ERROR.
    pub expected_errors: usize,
    /// Findings (only `Classification::Finding` reaches this list).
    pub findings: Vec<FindingEvidence>,
    /// Findings that were minimized under the configured strategy.
    pub minimized_findings: usize,
    /// Original declarations whose function was executed and evaluated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evaluated_declarations: Vec<String>,
    /// Functions not evaluated, with explicit reasons.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_functions: Vec<crate::finding::SkippedFunction>,
    /// True when at least one finding artifact is available on disk.
    pub artifacts_available: bool,
    /// Replay status per finding artifact, keyed by canonical hash.
    pub replay: BTreeMap<String, ReplayStatus>,
    /// True when the campaign's own determinism check passed (same seed ⇒
    /// same finding stream, verified by the campaign runner).
    pub deterministic: bool,
}

/// One finding as evidence: identity + reason + artifact reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingEvidence {
    /// Case id (deterministic, positional).
    pub case_id: String,
    /// Function invoked.
    pub function: String,
    /// Reason code name.
    pub reason_code: String,
    /// Canonical artifact hash (identity of the serialized finding).
    pub artifact_hash: String,
    /// True when minimization preserved the finding.
    pub minimized: bool,
    /// Minimization snapshot, including the accepted reduction trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimization: Option<crate::artifact::MinimizationSnapshot>,
}

impl CampaignEvidence {
    /// Build evidence from a completed campaign result.
    ///
    /// `artifacts` are the serialized findings the campaign wrote (or would
    /// write); `replay` maps artifact hash → outcome for artifacts that were
    /// replayed. `deterministic` is the campaign runner's own determinism
    /// verdict.
    pub fn from_result(
        result: &CampaignResult,
        config: crate::artifact::CampaignConfig,
        artifacts: &[FindingArtifact],
        replay: &BTreeMap<String, ReplayStatus>,
        deterministic: bool,
    ) -> Self {
        // The artifact hash is the canonical hash of the serialized finding,
        // not the wasm hash; match artifacts to findings by case id
        // (artifacts are produced in finding order).
        let mut by_case: BTreeMap<String, (String, crate::artifact::MinimizationSnapshot)> =
            BTreeMap::new();
        for a in artifacts {
            by_case.insert(
                a.case_id.clone(),
                (a.canonical_hash(), a.minimization.clone()),
            );
        }
        let findings: Vec<FindingEvidence> = result
            .findings
            .iter()
            .map(|f| FindingEvidence {
                case_id: f.identity.case_id.clone(),
                function: f.identity.function.clone(),
                reason_code: f.reason_code.name().to_string(),
                artifact_hash: by_case
                    .get(&f.identity.case_id)
                    .map(|(hash, _)| hash.clone())
                    .unwrap_or_default(),
                minimized: f.minimization.attempted && f.minimization.preserved,
                minimization: by_case
                    .get(&f.identity.case_id)
                    .map(|(_, minimization)| minimization.clone()),
            })
            .collect();

        Self {
            campaign_completed: result.executed > 0,
            config,
            case_count: result.executed,
            passed: result.passed.len(),
            expected_errors: result.expected_errors.len(),
            findings,
            minimized_findings: result.minimized_findings().count(),
            evaluated_declarations: result.evaluated_declarations.clone(),
            skipped_functions: result.skipped_functions.clone(),
            artifacts_available: !artifacts.is_empty(),
            replay: replay.clone(),
            deterministic,
        }
    }

    /// True when the campaign produced at least one finding.
    pub fn has_findings(&self) -> bool {
        !self.findings.is_empty()
    }

    /// True when every finding artifact replayed and reproduced.
    pub fn all_reproduced(&self) -> bool {
        !self.replay.is_empty()
            && self
                .replay
                .values()
                .all(|s| matches!(s, ReplayStatus::Reproduced { .. }))
    }
}

/// Record a replay outcome into a status map (helper for the CLI/evidence
/// path).
pub fn record_replay(
    map: &mut BTreeMap<String, ReplayStatus>,
    artifact_hash: &str,
    outcome: &ReplayOutcome,
) {
    let status = match outcome {
        ReplayOutcome::Reproduced { reason_code, .. } => ReplayStatus::Reproduced {
            reason_code: reason_code.name().to_string(),
        },
        ReplayOutcome::Mismatch { expected, .. } => ReplayStatus::Mismatch {
            reason_code: expected.name().to_string(),
        },
    };
    map.insert(artifact_hash.to_string(), status);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::CampaignConfig;

    #[test]
    fn zero_executed_campaign_is_not_marked_completed() {
        let result = CampaignResult::default();
        let evidence = CampaignEvidence::from_result(
            &result,
            CampaignConfig::default(),
            &[],
            &BTreeMap::new(),
            true,
        );
        assert!(!evidence.campaign_completed);
        assert_eq!(evidence.case_count, 0);
        assert!(evidence.findings.is_empty());
    }

    #[test]
    fn nonzero_executed_campaign_is_marked_completed() {
        let result = CampaignResult {
            executed: 2,
            passed: vec!["case0000".to_string(), "case0001".to_string()],
            ..Default::default()
        };
        let evidence = CampaignEvidence::from_result(
            &result,
            CampaignConfig::default(),
            &[],
            &BTreeMap::new(),
            true,
        );
        assert!(evidence.campaign_completed);
        assert_eq!(evidence.case_count, 2);
        assert_eq!(evidence.passed, 2);
    }
}
