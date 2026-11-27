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

/// Current schema version for campaign evidence documents.
pub const CAMPAIGN_EVIDENCE_SCHEMA_VERSION: u32 = 1;

fn legacy_schema_version() -> u32 {
    0
}

fn unknown_producer() -> String {
    "unknown".to_string()
}

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
    /// True when the run declared budget-class failures as RESOURCE_LIMIT
    /// findings (`--resource-limit-is-finding`). Opt-in; default off, so
    /// default-run evidence omits the field entirely (byte-identical to
    /// pre-flag evidence) and a run that used the rule is distinguishable
    /// from one that did not.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resource_limit_is_finding: bool,
    /// Campaign evidence document schema version (0 means a legacy document
    /// with no schema version field).
    #[serde(default = "legacy_schema_version")]
    pub schema_version: u32,
    /// CLI name that produced the campaign evidence.
    #[serde(default = "unknown_producer")]
    pub tool: String,
    /// Tool package version that produced the campaign evidence.
    #[serde(default = "unknown_producer")]
    pub version: String,
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
        resource_limit_is_finding: bool,
    ) -> Self {
        // The artifact hash is the canonical hash of the serialized finding,
        // not the wasm hash; match artifacts to findings by case id
        // (artifacts are produced in finding order).
        let mut by_case: BTreeMap<String, String> = BTreeMap::new();
        for a in artifacts {
            by_case.insert(a.case_id.clone(), a.canonical_hash());
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
                    .cloned()
                    .unwrap_or_default(),
                minimized: f.minimization.attempted && f.minimization.preserved,
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
            resource_limit_is_finding,
            schema_version: CAMPAIGN_EVIDENCE_SCHEMA_VERSION,
            tool: "sdkt".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
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
            false,
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
            false,
        );
        assert!(evidence.campaign_completed);
        assert_eq!(evidence.case_count, 2);
        assert_eq!(evidence.passed, 2);
    }

    #[test]
    fn legacy_evidence_without_provenance_still_deserializes() {
        let result = CampaignResult {
            executed: 1,
            passed: vec!["case0000".to_string()],
            ..Default::default()
        };
        let evidence = CampaignEvidence::from_result(
            &result,
            CampaignConfig::default(),
            &[],
            &BTreeMap::new(),
            true,
            false,
        );
        let mut legacy = serde_json::to_value(evidence).unwrap();
        legacy.as_object_mut().unwrap().remove("schema_version");
        legacy.as_object_mut().unwrap().remove("tool");
        legacy.as_object_mut().unwrap().remove("version");

        let parsed: CampaignEvidence = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.schema_version, 0);
        assert_eq!(parsed.tool, "unknown");
        assert_eq!(parsed.version, "unknown");
    }
}
