//! Campaign runner: the CLI-facing business logic (Phase 3).
//!
//! Everything the `sdkt fuzz` command needs to do lives here — parsing,
//! campaign execution, artifact writing, replay, and release-assurance
//! evidence. The CLI stays a thin adapter: flags in, one call, report out.
//!
//! Exit-code semantics are the CLI's decision, not this module's; the
//! [`FuzzReport`] exposes exactly what happened (`findings` present, replay
//! counts) so the adapter can map them without re-deriving anything.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::artifact::{encode_scval_xdr_hex, FindingArtifact};
use crate::auth::AuthMode;
use crate::campaign::{run_campaign_artifacts, summarize, CampaignInput, CampaignResult};
use crate::environment::Environment;
use crate::error::FuzzError;
use crate::evidence::{CampaignEvidence, ReplayStatus};
use crate::oracle::{Expected, ExpectedBehavior};
use crate::replay::{parse_artifact, replay, ReplayError, ReplayOutcome};
use soroban_env_host::xdr::ScVal;

/// Parsed options for one `sdkt fuzz <wasm>` run.
#[derive(Clone, Debug)]
pub struct FuzzRunOptions {
    pub wasm: PathBuf,
    /// u64 seed, expanded deterministically into the 32-byte host seed.
    pub seed: u64,
    pub cases: usize,
    pub function: Option<String>,
    pub artifact_dir: Option<PathBuf>,
    pub auth_mode: AuthMode,
    /// `--expect-success FN` declarations.
    pub expect_success: Vec<String>,
    /// `--expect-error FN:TYPE:CODE` declarations.
    pub expect_error: Vec<String>,
    /// Replay every written artifact (deterministic verification).
    pub replay: bool,
}

impl Default for FuzzRunOptions {
    fn default() -> Self {
        Self {
            wasm: PathBuf::new(),
            seed: 0,
            cases: 100,
            function: None,
            artifact_dir: None,
            auth_mode: AuthMode::NoAuth,
            expect_success: Vec::new(),
            expect_error: Vec::new(),
            replay: false,
        }
    }
}

/// Parsed options for one `sdkt fuzz replay <artifact>` run.
#[derive(Clone, Debug)]
pub struct ReplayOptions {
    pub artifact: PathBuf,
    pub wasm: PathBuf,
}

/// What one campaign run produced.
#[derive(Debug)]
pub struct FuzzReport {
    pub summary: String,
    pub result: CampaignResult,
    /// (file_name, canonical_hash) per written artifact.
    pub artifacts: Vec<(String, String)>,
    /// Replay statuses keyed by artifact canonical hash, when requested.
    pub replay: BTreeMap<String, ReplayStatus>,
    pub evidence: CampaignEvidence,
}

impl FuzzReport {
    /// Pretty rendering (human output; canonical JSON is the evidence file).
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Campaign summary: {}\n", self.summary));
        for f in &self.result.findings {
            out.push_str(&format!(
                "FINDING {} {} {} (minimized_complexity={})\n",
                f.identity.case_id,
                f.identity.function,
                f.reason_code.name(),
                f.minimization.minimized_complexity
            ));
        }
        for (name, hash) in &self.artifacts {
            out.push_str(&format!("artifact {name} canonical_hash={hash}\n"));
        }
        for (hash, status) in &self.replay {
            out.push_str(&format!("replay {hash} {:?}\n", status_kind(status)));
        }
        out.push_str(&format!(
            "evidence: completed={} findings={} minimized={} replayed={} deterministic={}\n",
            self.evidence.campaign_completed,
            self.evidence.findings.len(),
            self.evidence.minimized_findings,
            self.evidence.replay.len(),
            self.evidence.deterministic,
        ));
        out
    }

    /// Canonical JSON evidence (release-assurance consumption format).
    pub fn to_json(&self) -> Result<String, FuzzError> {
        serde_json::to_string(&self.evidence).map_err(|e| FuzzError::Observation(e.to_string()))
    }

    /// JSONL: one line per finding, plus a final summary line.
    pub fn to_jsonl(&self) -> Result<String, FuzzError> {
        let mut out = String::new();
        for f in &self.result.findings {
            // Findings are rich runtime structures; the JSONL stream carries
            // the evidence-side projection of each finding.
            let projection = serde_json::json!({
                "case_id": f.identity.case_id,
                "function": f.identity.function,
                "reason_code": f.reason_code.name(),
                "auth_mode": f.identity.auth_mode,
                "mutation_id": f.identity.mutation_id,
                "minimized": f.minimization.attempted && f.minimization.preserved,
            });
            out.push_str(
                &serde_json::to_string(&projection)
                    .map_err(|e| FuzzError::Observation(e.to_string()))?,
            );
            out.push('\n');
        }
        out.push_str(
            &serde_json::to_string(&self.evidence)
                .map_err(|e| FuzzError::Observation(e.to_string()))?,
        );
        out.push('\n');
        Ok(out)
    }
}

fn status_kind(s: &ReplayStatus) -> &'static str {
    match s {
        ReplayStatus::NotReplayed => "not_replayed",
        ReplayStatus::Reproduced { .. } => "reproduced",
        ReplayStatus::Mismatch { .. } => "mismatch",
        ReplayStatus::Error { .. } => "error",
    }
}

/// Run a campaign: read the WASM, generate/mutate/execute/classify/minimize,
/// write one artifact per finding, optionally replay them, and build the
/// release-assurance evidence.
pub fn run(options: &FuzzRunOptions) -> Result<FuzzReport, FuzzError> {
    let wasm = std::fs::read(&options.wasm).map_err(|e| {
        FuzzError::InvalidConfig(format!("reading {}: {e}", options.wasm.display()))
    })?;

    let mut input = CampaignInput::new(&wasm, expand_seed(options.seed))?;
    input.cases = options.cases;
    input.function = options.function.clone();
    input.auth_mode = options.auth_mode;
    input.environment = Environment::default();

    let expectations = build_expectations(options)?;
    let (result, artifacts) = run_campaign_artifacts(&input, &expectations)?;

    // Artifacts: write only actual Classification::Finding findings.
    let dir = match &options.artifact_dir {
        Some(d) => d.clone(),
        None => std::env::current_dir()
            .map_err(|e| FuzzError::InvalidConfig(format!("cwd: {e}")))?
            .join("sdkt-fuzz-artifacts"),
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| FuzzError::InvalidConfig(format!("creating artifact dir: {e}")))?;

    let mut written: Vec<(String, String)> = Vec::new();
    let mut written_artifacts: Vec<FindingArtifact> = Vec::new();
    for art in &artifacts {
        let name = artifact_file_name(art);
        let path = dir.join(&name);
        std::fs::write(&path, format!("{}\n", art.canonical_json()))
            .map_err(|e| FuzzError::InvalidConfig(format!("writing artifact: {e}")))?;
        written.push((name, art.canonical_hash()));
        written_artifacts.push(art.clone());
    }

    // Deterministic verification via replay (opt-in).
    let mut replay_map: BTreeMap<String, ReplayStatus> = BTreeMap::new();
    if options.replay {
        for art in &written_artifacts {
            match replay(art, &wasm) {
                Ok(outcome) => {
                    crate::evidence::record_replay(&mut replay_map, &art.canonical_hash(), &outcome)
                }
                Err(e) => {
                    replay_map.insert(
                        art.canonical_hash(),
                        ReplayStatus::Error {
                            reason: e.to_string(),
                        },
                    );
                }
            }
        }
    }

    // Determinism verdict: re-run the same campaign and compare the artifact
    // stream. Cheap enough for the CLI's default case budgets.
    let deterministic = determinism_check(&wasm, &input, &expectations, &artifacts)?;

    let evidence = CampaignEvidence::from_result(
        &result,
        input.artifact_config(),
        &written_artifacts,
        &replay_map,
        deterministic,
    );

    // Write the evidence file next to the artifacts (release-assurance input).
    let evidence_path = dir.join("campaign-evidence.json");
    std::fs::write(
        &evidence_path,
        serde_json::to_string_pretty(&evidence)
            .map_err(|e| FuzzError::Observation(e.to_string()))?,
    )
    .map_err(|e| FuzzError::InvalidConfig(format!("writing evidence: {e}")))?;

    Ok(FuzzReport {
        summary: summarize(&result),
        result,
        artifacts: written,
        replay: replay_map,
        evidence,
    })
}

/// Replay one artifact against a WASM file.
pub fn run_replay(options: &ReplayOptions) -> Result<ReplayOutcome, ReplayError> {
    let json = std::fs::read(&options.artifact).map_err(|e| {
        ReplayError::Malformed(format!("reading {}: {e}", options.artifact.display()))
    })?;
    let wasm = std::fs::read(&options.wasm)
        .map_err(|e| ReplayError::Malformed(format!("reading {}: {e}", options.wasm.display())))?;
    let artifact = parse_artifact(&json)?;
    replay(&artifact, &wasm)
}

/// Render a replay outcome as a single canonical line (stable across
/// processes; the CLI adapter prints it verbatim).
pub fn replay_line(outcome: &ReplayOutcome) -> String {
    match outcome {
        ReplayOutcome::Reproduced {
            reason_code,
            observation_hash,
            artifact_hash,
        } => format!(
            "REPRODUCED reason={} artifact_hash={artifact_hash} observation_hash={observation_hash} canonical={}",
            reason_code.name(),
            outcome.canonical_hash()
        ),
        ReplayOutcome::Mismatch {
            expected,
            actual,
            artifact_hash,
            observation_hash,
        } => format!(
            "MISMATCH expected={expected:?} actual={actual:?} artifact_hash={artifact_hash} observation_hash={observation_hash} canonical={}",
            outcome.canonical_hash()
        ),
    }
}

/// Deterministic artifact file name for a finding (case_id is positional
/// and unique; reason disambiguates re-issues).
pub fn artifact_file_name(art: &FindingArtifact) -> String {
    format!(
        "finding-{}-{}.json",
        art.case_id,
        art.reason_code.to_lowercase()
    )
}

/// u64 CLI seed → 32-byte host seed (little-endian + fixed fill: stable and
/// reversible-by-reading-the-artifact).
pub fn expand_seed(seed: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    bytes
}

/// Parse `--expect-success` / `--expect-error FN:TYPE:CODE` into the
/// expectations map. Malformed declarations are configuration errors, not
/// findings.
fn build_expectations(options: &FuzzRunOptions) -> Result<BTreeMap<String, Expected>, FuzzError> {
    let mut map = BTreeMap::new();
    for f in &options.expect_success {
        map.insert(
            f.clone(),
            Expected {
                behavior: ExpectedBehavior::Success {
                    expect_return: None,
                },
                ..Default::default()
            },
        );
    }
    for decl in &options.expect_error {
        let mut parts = decl.splitn(3, ':');
        let (Some(function), Some(error_type), Some(code)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return Err(FuzzError::InvalidConfig(format!(
                "--expect-error expects FN:TYPE:CODE, got `{decl}`"
            )));
        };
        let code: u32 = code.parse().map_err(|e| {
            FuzzError::InvalidConfig(format!("--expect-error `{decl}`: bad code: {e}"))
        })?;
        map.insert(
            function.to_string(),
            Expected {
                behavior: ExpectedBehavior::Error {
                    error_type: error_type.to_string(),
                    code,
                },
                ..Default::default()
            },
        );
    }
    Ok(map)
}

/// Re-run the campaign and compare artifact streams byte-for-byte. The
/// campaign is deterministic by construction; this is the CLI's evidence
/// assertion, not a new mechanism.
fn determinism_check(
    wasm: &[u8],
    input: &CampaignInput,
    expectations: &BTreeMap<String, Expected>,
    artifacts: &[FindingArtifact],
) -> Result<bool, FuzzError> {
    let mut replay_input = CampaignInput::new(wasm, input.seed)?;
    replay_input.cases = input.cases;
    replay_input.function = input.function.clone();
    replay_input.auth_mode = input.auth_mode;
    replay_input.environment = input.environment.clone();
    replay_input.sequence_length = input.sequence_length;
    replay_input.sequence_every = input.sequence_every;
    replay_input.mutations_per_case = input.mutations_per_case;
    let (_, second) = run_campaign_artifacts(&replay_input, expectations)?;
    Ok(second
        .iter()
        .map(|a| a.canonical_hash())
        .collect::<Vec<_>>()
        == artifacts
            .iter()
            .map(|a| a.canonical_hash())
            .collect::<Vec<_>>())
}

/// Load and validate an artifact from disk (used by tests + the CLI).
pub fn load_artifact(path: &Path) -> Result<FindingArtifact, ReplayError> {
    let json = std::fs::read(path)
        .map_err(|e| ReplayError::Malformed(format!("reading {}: {e}", path.display())))?;
    parse_artifact(&json)
}

/// Convenience for CLI/tests: the ScVal hex of a U32 (expected-return
/// declarations need the canonical encoding once).
pub fn u32_scval_hex(n: u32) -> String {
    encode_scval_xdr_hex(&ScVal::U32(n))
}
