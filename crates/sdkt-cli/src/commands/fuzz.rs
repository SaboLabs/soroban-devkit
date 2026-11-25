//! `sdkt fuzz` — the CLI adapter for the fuzz engine.
//!
//! Thin by construction: parse flags, call `sdkt_fuzz`, print, map the
//! result to an exit code. No campaign logic, no oracle, no artifact format
//! decisions live here.
//!
//! Exit-code contract (distinct paths, documented):
//!
//! - `0` — campaign completed with no findings (or replay reproduced);
//! - `1` — actual `Classification::Finding` findings exist (campaign), or
//!   replay did **not** reproduce the recorded finding;
//! - `2` — usage / input error: bad flags, unreadable files, invalid
//!   artifact, or a WASM whose SHA-256 does not match the artifact.
//!
//! Exit `1` is the only path that means "findings were produced".

use sdkt_fuzz::{AuthMode, FuzzRunOptions, ReplayOptions, SequenceOptions};

/// Output formats supported by the fuzz surface.
pub enum FuzzFormat {
    Pretty,
    Json,
    Jsonl,
}

/// Parse `--format`. `None` means a usage error (exit 2), never a finding.
pub fn parse_format(s: &str) -> Option<FuzzFormat> {
    match s.to_lowercase().as_str() {
        "pretty" => Some(FuzzFormat::Pretty),
        "json" => Some(FuzzFormat::Json),
        "jsonl" => Some(FuzzFormat::Jsonl),
        _ => None,
    }
}

/// Parse the `--auth` flag into the engine's auth mode.
pub fn parse_auth(mode: &str) -> Option<AuthMode> {
    match mode.to_lowercase().as_str() {
        "no-auth" | "noauth" | "none" => Some(AuthMode::NoAuth),
        "correct-auth" | "correct" | "correctauth" => Some(AuthMode::CorrectAuth),
        "wrong-auth" | "wrong" | "wrongauth" => Some(AuthMode::WrongAuth),
        _ => None,
    }
}

/// Validate `--sequence-length` and `--sequence-every` as a pair.
///
/// Rules:
/// - Omitting both flags keeps the default single-step behavior
///   (`sequence_length = 2`, `sequence_every = 0`).
/// - Supplying only one of the two flags is a usage error.
/// - `sequence_length` must be `>= 2` (a sequence needs at least 2 steps to
///   carry state between invocations).
/// - `sequence_every` must be `>= 1` (`0` is the internal disabled sentinel
///   and cannot be passed on the CLI).
pub fn validate_sequence_flags(
    sequence_length: Option<usize>,
    sequence_every: Option<usize>,
) -> Result<SequenceOptions, String> {
    if let Some(len) = sequence_length {
        if len < 2 {
            return Err(format!(
                "invalid --sequence-length {len}: must be >= 2 (a sequence requires at least 2 steps; pass both --sequence-length <N> (>= 2) and --sequence-every <N> (>= 1))"
            ));
        }
    }
    if let Some(every) = sequence_every {
        if every < 1 {
            return Err(format!(
                "invalid --sequence-every {every}: must be >= 1 (pass both --sequence-length <N> (>= 2) and --sequence-every <N> (>= 1))"
            ));
        }
    }
    match (sequence_length, sequence_every) {
        (None, None) => Ok(SequenceOptions::default()),
        (Some(_), None) => Err(
            "--sequence-length requires --sequence-every (pass both --sequence-length <N> (>= 2) and --sequence-every <N> (>= 1), or omit both)".to_string(),
        ),
        (None, Some(_)) => Err(
            "--sequence-every requires --sequence-length (pass both --sequence-length <N> (>= 2) and --sequence-every <N> (>= 1), or omit both)".to_string(),
        ),
        (Some(sequence_length), Some(sequence_every)) => Ok(SequenceOptions {
            sequence_length,
            sequence_every,
        }),
    }
}

/// Parsed TOML campaign definition (`sdkt fuzz campaign --config <FILE>`).
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FuzzCampaignConfigFile {
    #[serde(default)]
    pub campaign: CampaignSection,
    #[serde(default)]
    pub expect: ExpectSection,
}

/// `[campaign]` table in a campaign TOML config.
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CampaignSection {
    pub wasm: Option<String>,
    pub seed: Option<u64>,
    pub cases: Option<usize>,
    pub function: Option<String>,
    pub auth: Option<String>,
}

/// `[expect]` container for `[[expect.success]]` and `[[expect.error]]`.
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ExpectSection {
    #[serde(default)]
    pub success: Vec<ExpectSuccessEntry>,
    #[serde(default)]
    pub error: Vec<ExpectErrorEntry>,
}

/// One `[[expect.success]]` entry (`function = "..."`).
#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectSuccessEntry {
    pub function: String,
}

/// One `[[expect.error]]` entry (`function = "..."`, `type = "..."`, `code = N`).
#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectErrorEntry {
    pub function: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: u32,
}

/// Resolved campaign parameters after merging CLI flags > config > defaults.
#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedCampaignInputs {
    pub wasm: String,
    pub seed: u64,
    pub cases: usize,
    pub function: Option<String>,
    pub auth: String,
    pub expect_success: Vec<String>,
    pub expect_error: Vec<String>,
}

/// Parse a TOML campaign config string, naming `path` on any error.
pub fn parse_campaign_config(text: &str, path: &str) -> Result<FuzzCampaignConfigFile, String> {
    toml::from_str(text).map_err(|e| format!("cannot parse campaign config '{path}': {e}"))
}

/// Read and parse a TOML campaign config file, naming `path` on any error.
pub fn read_campaign_config(path: &str) -> Result<FuzzCampaignConfigFile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read campaign config '{path}': {e}"))?;
    parse_campaign_config(&text, path)
}

/// Merge CLI flags, optional config file, and built-in defaults.
///
/// Precedence: explicit CLI flags > config values > defaults.
/// Positional `wasm` and `campaign.wasm` must agree when both are supplied.
#[allow(clippy::too_many_arguments)]
pub fn resolve_campaign_inputs(
    cli_wasm: Option<String>,
    config_path: Option<&str>,
    cli_seed: Option<u64>,
    cli_cases: Option<usize>,
    cli_function: Option<String>,
    cli_auth: Option<String>,
    cli_expect_success: Vec<String>,
    cli_expect_error: Vec<String>,
) -> Result<ResolvedCampaignInputs, String> {
    let cfg = match config_path {
        Some(path) => Some((path, read_campaign_config(path)?)),
        None => None,
    };

    let wasm = match (
        cli_wasm,
        cfg.as_ref().and_then(|(_, c)| c.campaign.wasm.as_ref()),
    ) {
        (Some(pos), Some(cfg_wasm)) => {
            if pos != *cfg_wasm {
                let path = cfg.as_ref().map(|(p, _)| *p).unwrap_or("");
                return Err(format!(
                    "conflicting WASM paths: positional argument '{pos}' does not match config '{cfg_wasm}' in '{path}'"
                ));
            }
            pos
        }
        (Some(pos), None) => pos,
        (None, Some(cfg_wasm)) => cfg_wasm.clone(),
        (None, None) => {
            return match config_path {
                Some(path) => Err(format!(
                    "missing WASM path: neither positional <WASM> nor 'campaign.wasm' in '{path}' was provided"
                )),
                None => Err(
                    "missing required <WASM> argument (pass <WASM> or --config <FILE>)".to_string(),
                ),
            };
        }
    };

    let cfg_file = cfg.map(|(_, c)| c).unwrap_or_default();
    let seed = cli_seed.or(cfg_file.campaign.seed).unwrap_or(0);
    let cases = cli_cases.or(cfg_file.campaign.cases).unwrap_or(100);
    let function = cli_function.or(cfg_file.campaign.function);
    let auth = cli_auth
        .or(cfg_file.campaign.auth)
        .unwrap_or_else(|| "no-auth".to_string());

    let expect_success = if !cli_expect_success.is_empty() {
        cli_expect_success
    } else {
        cfg_file
            .expect
            .success
            .into_iter()
            .map(|e| e.function)
            .collect()
    };

    let expect_error = if !cli_expect_error.is_empty() {
        cli_expect_error
    } else {
        cfg_file
            .expect
            .error
            .into_iter()
            .map(|e| format!("{}:{}:{}", e.function, e.error_type, e.code))
            .collect()
    };

    Ok(ResolvedCampaignInputs {
        wasm,
        seed,
        cases,
        function,
        auth,
        expect_success,
        expect_error,
    })
}

/// Build campaign options from the CLI flags.
#[allow(clippy::too_many_arguments)]
pub fn options_from(
    wasm: &str,
    seed: u64,
    cases: usize,
    function: Option<String>,
    artifact_dir: Option<String>,
    auth: &str,
    expect_success: Vec<String>,
    expect_error: Vec<String>,
    resource_limit_is_finding: bool,
    budget_capped: Option<String>,
    replay: bool,
) -> Result<FuzzRunOptions, String> {
    let auth_mode = parse_auth(auth).ok_or_else(|| {
        format!("invalid auth mode '{auth}' (expected no-auth, correct-auth, wrong-auth)")
    })?;
    let budget = parse_budget_capped(budget_capped.as_deref()).ok_or_else(|| {
        format!("invalid --budget-capped value '{budget_capped:?}' (expected CPU:MEM, e.g. 1:1)")
    })?;
    Ok(FuzzRunOptions {
        wasm: std::path::PathBuf::from(wasm),
        seed,
        cases,
        function,
        artifact_dir: artifact_dir.map(std::path::PathBuf::from),
        auth_mode,
        expect_success,
        expect_error,
        resource_limit_is_finding,
        budget_capped: budget,
        replay,
    })
}

/// Parse `--budget-capped CPU:MEM` into deterministic per-case ceilings.
pub fn parse_budget_capped(spec: Option<&str>) -> Option<Option<(u64, u64)>> {
    let Some(spec) = spec else {
        return Some(None);
    };
    let mut parts = spec.splitn(2, ':');
    let (Some(cpu), Some(mem)) = (parts.next(), parts.next()) else {
        return None;
    };
    let (cpu, mem) = (cpu.parse::<u64>().ok()?, mem.parse::<u64>().ok()?);
    Some(Some((cpu, mem)))
}

/// Run one campaign and render it. `Ok(true)` = findings produced, `Ok(false)`
/// = clean run; `Err(_)` = usage/input error.
//
/// Resolves `--config` (if supplied) with CLI flags overriding config values
/// and config values overriding built-in defaults, then delegates to the
/// existing campaign runner unchanged.
#[allow(clippy::too_many_arguments)]
pub fn run(
    wasm: Option<String>,
    config: Option<String>,
    seed: Option<u64>,
    cases: Option<usize>,
    function: Option<String>,
    artifact_dir: Option<String>,
    auth: Option<String>,
    expect_success: Vec<String>,
    expect_error: Vec<String>,
    sequence_length: Option<usize>,
    sequence_every: Option<usize>,
    resource_limit_is_finding: bool,
    budget_capped: Option<String>,
    replay: bool,
    format: &str,
) -> Result<bool, String> {
    let resolved = resolve_campaign_inputs(
        wasm,
        config.as_deref(),
        seed,
        cases,
        function,
        auth,
        expect_success,
        expect_error,
    )?;
    let options = options_from(
        &resolved.wasm,
        resolved.seed,
        resolved.cases,
        resolved.function,
        artifact_dir,
        &resolved.auth,
        resolved.expect_success,
        resolved.expect_error,
        resource_limit_is_finding,
        budget_capped,
        replay,
    )?;
    let sequence = validate_sequence_flags(sequence_length, sequence_every)?;
    let fmt = parse_format(format)
        .ok_or_else(|| format!("invalid format '{format}' (expected pretty, json, or jsonl)"))?;

    let report =
        sdkt_fuzz::run_fuzz_with_sequence(&options, sequence).map_err(|e| e.to_string())?;
    print_report(&report, &fmt);
    Ok(!report.result.findings.is_empty())
}

/// Replay one artifact and render the verdict.
///
/// `Ok(true)` = reproduced; `Ok(false)` = valid inputs but the replay
/// mismatched; `Err(_)` = invalid artifact/WASM (usage/input error).
pub fn run_replay(artifact: &str, wasm: &str, format: &str) -> Result<bool, String> {
    let fmt = parse_format(format)
        .ok_or_else(|| format!("invalid format '{format}' (expected pretty, json, or jsonl)"))?;
    let options = ReplayOptions {
        artifact: std::path::PathBuf::from(artifact),
        wasm: std::path::PathBuf::from(wasm),
    };
    let outcome = sdkt_fuzz::run_replay(&options).map_err(|e| e.to_string())?;
    print_replay(&outcome, &fmt);
    Ok(outcome.is_reproduced())
}

/// Compare two campaign evidence files and render the verdict.
///
/// `Ok(true)` = the new run introduced at least one finding (exit 1);
/// `Ok(false)` = no new findings, including the all-fixed case (exit 0);
/// `Err(_)` = usage/input error: unreadable or unparseable evidence file,
/// or campaigns for different WASM artifacts (exit 2).
///
/// Read-only: neither evidence file is written, modified, or deleted.
pub fn run_compare(baseline: &str, new: &str, format: &str) -> Result<bool, String> {
    let fmt = parse_format(format)
        .ok_or_else(|| format!("invalid format '{format}' (expected pretty, json, or jsonl)"))?;
    // `jsonl` has no meaning for a comparison report; treat it as `json`.
    let fmt = match fmt {
        FuzzFormat::Jsonl => FuzzFormat::Json,
        other => other,
    };

    let baseline_evidence = read_evidence(baseline)?;
    let new_evidence = read_evidence(new)?;
    let comparison = sdkt_fuzz::compare_evidence(&baseline_evidence, &new_evidence)?;

    match fmt {
        FuzzFormat::Pretty => print_comparison_pretty(
            baseline,
            &baseline_evidence,
            new,
            &new_evidence,
            &comparison,
        ),
        FuzzFormat::Json => print_comparison_json(&comparison),
        FuzzFormat::Jsonl => unreachable!("jsonl is normalized to json above"),
    }

    Ok(comparison.has_new_findings())
}

/// Read and parse one campaign evidence file, naming the file on failure.
fn read_evidence(path: &str) -> Result<sdkt_fuzz::CampaignEvidence, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read evidence file '{path}': {e}"))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse evidence file '{path}' as CampaignEvidence: {e}"))
}

fn print_comparison_pretty(
    baseline_path: &str,
    baseline: &sdkt_fuzz::CampaignEvidence,
    new_path: &str,
    new: &sdkt_fuzz::CampaignEvidence,
    comparison: &sdkt_fuzz::EvidenceComparison,
) {
    let (new_count, fixed_count, persisting_count) = comparison.counts();
    println!("Fuzz compare");
    println!(
        "  Baseline: {} (wasm_sha256 {}, cases {}, seed {})",
        baseline_path, baseline.config.wasm_sha256, baseline.config.cases, baseline.config.seed_hex
    );
    println!(
        "  New:      {} (wasm_sha256 {}, cases {}, seed {})",
        new_path, new.config.wasm_sha256, new.config.cases, new.config.seed_hex
    );
    println!("  New: {new_count} | Fixed: {fixed_count} | Persisting: {persisting_count}");
    print_comparison_findings("New findings", &comparison.new);
    print_comparison_findings("Fixed findings", &comparison.fixed);
    if !comparison.persisting.is_empty() {
        println!();
        println!("Persisting findings ({}):", comparison.persisting.len());
        for f in &comparison.persisting {
            println!(
                "  - {} {} {} ({})",
                f.case_id, f.function, f.reason_code, f.artifact_hash
            );
        }
    }
}

fn print_comparison_findings(heading: &str, findings: &[sdkt_fuzz::evidence::FindingEvidence]) {
    if findings.is_empty() {
        return;
    }
    println!();
    println!("{heading} ({}):", findings.len());
    for f in findings {
        println!(
            "  - {} {} {} ({})",
            f.case_id, f.function, f.reason_code, f.artifact_hash
        );
    }
}

fn print_comparison_json(comparison: &sdkt_fuzz::EvidenceComparison) {
    let (new_count, fixed_count, persisting_count) = comparison.counts();
    let json = serde_json::json!({
        "baseline_wasm_sha256": comparison.baseline_wasm_sha256,
        "new_wasm_sha256": comparison.new_wasm_sha256,
        "summary": {
            "new": new_count,
            "fixed": fixed_count,
            "persisting": persisting_count,
        },
        "new": comparison.new,
        "fixed": comparison.fixed,
        "persisting": comparison.persisting,
    });
    println!(
        "{}",
        serde_json::to_string(&json).expect("comparison view serializes")
    );
}

/// Render a campaign report.
fn print_report(report: &sdkt_fuzz::FuzzReport, fmt: &FuzzFormat) {
    match fmt {
        FuzzFormat::Json => {
            let json = report.to_json().unwrap_or_else(|e| {
                eprintln!("Error serializing report: {e}");
                std::process::exit(1);
            });
            println!("{json}");
        }
        FuzzFormat::Jsonl => {
            let lines = report.to_jsonl().unwrap_or_else(|e| {
                eprintln!("Error serializing report: {e}");
                std::process::exit(1);
            });
            print!("{lines}");
        }
        FuzzFormat::Pretty => print!("{}", report.to_pretty()),
    }
}

/// Render a replay verdict.
fn print_replay(outcome: &sdkt_fuzz::ReplayOutcome, fmt: &FuzzFormat) {
    match fmt {
        FuzzFormat::Pretty => println!("{}", sdkt_fuzz::replay_line(outcome)),
        FuzzFormat::Json | FuzzFormat::Jsonl => {
            let json = replay_json_value(outcome);
            println!(
                "{}",
                serde_json::to_string(&json).expect("replay view serializes")
            );
        }
    }
}

fn replay_json_value(outcome: &sdkt_fuzz::ReplayOutcome) -> serde_json::Value {
    use sdkt_fuzz::ReplayOutcome::{Mismatch, Reproduced};
    let mut v = serde_json::json!({
        "canonical_hash": outcome.canonical_hash(),
    });
    match outcome {
        Reproduced {
            reason_code,
            observation_hash,
            artifact_hash,
        } => {
            v["result"] = serde_json::json!("reproduced");
            v["reason_code"] = serde_json::json!(reason_code.name());
            v["artifact_hash"] = serde_json::json!(artifact_hash);
            v["observation_hash"] = serde_json::json!(observation_hash);
        }
        Mismatch {
            expected,
            actual,
            observation_hash,
            artifact_hash,
        } => {
            v["result"] = serde_json::json!("mismatch");
            v["expected_reason_code"] = serde_json::json!(expected.name());
            v["actual_classification"] = serde_json::json!(format!("{actual:?}"));
            v["artifact_hash"] = serde_json::json!(artifact_hash);
            v["observation_hash"] = serde_json::json!(observation_hash);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_config(name: &str, contents: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sdkt-fuzz-config-unit-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("fuzz.toml");
        std::fs::write(&path, contents).expect("write temp config");
        path
    }

    #[test]
    fn parse_valid_campaign_config_with_expectations() {
        let toml = r#"
[campaign]
wasm = "contract.wasm"
seed = 42
cases = 50
function = "increment"
auth = "correct-auth"

[[expect.success]]
function = "increment"

[[expect.error]]
function = "withdraw"
type = "Auth"
code = 6
"#;
        let parsed = parse_campaign_config(toml, "fuzz.toml").expect("valid config");
        assert_eq!(parsed.campaign.wasm.as_deref(), Some("contract.wasm"));
        assert_eq!(parsed.campaign.seed, Some(42));
        assert_eq!(parsed.campaign.cases, Some(50));
        assert_eq!(parsed.campaign.function.as_deref(), Some("increment"));
        assert_eq!(parsed.campaign.auth.as_deref(), Some("correct-auth"));
        assert_eq!(
            parsed.expect.success,
            vec![ExpectSuccessEntry {
                function: "increment".to_string()
            }]
        );
        assert_eq!(
            parsed.expect.error,
            vec![ExpectErrorEntry {
                function: "withdraw".to_string(),
                error_type: "Auth".to_string(),
                code: 6,
            }]
        );
    }

    #[test]
    fn reject_unknown_key_naming_key_and_file() {
        let toml = r#"
[campaign]
wasm = "contract.wasm"
bogus_option = 123
"#;
        let err = parse_campaign_config(toml, "my-fuzz.toml").unwrap_err();
        assert!(
            err.contains("bogus_option"),
            "error should name unknown key: {err}"
        );
        assert!(
            err.contains("my-fuzz.toml"),
            "error should name config file: {err}"
        );
    }

    #[test]
    fn reject_missing_and_malformed_config_naming_file() {
        let missing_err = read_campaign_config("/nonexistent/path/missing-fuzz.toml").unwrap_err();
        assert!(
            missing_err.contains("missing-fuzz.toml"),
            "error should name missing file: {missing_err}"
        );

        let bad_err = parse_campaign_config("[campaign\nwasm = ", "broken.toml").unwrap_err();
        assert!(
            bad_err.contains("broken.toml"),
            "error should name malformed file: {bad_err}"
        );
    }

    #[test]
    fn precedence_flags_override_config_override_defaults() {
        let cfg_path = write_temp_config(
            "precedence",
            r#"
[campaign]
wasm = "from_config.wasm"
seed = 42
cases = 50
function = "from_cfg_fn"
auth = "correct-auth"

[[expect.success]]
function = "cfg_ok"

[[expect.error]]
function = "cfg_err"
type = "Auth"
code = 6
"#,
        );
        let cfg_str = cfg_path.to_str().unwrap();

        // 1. Config values override built-in defaults when CLI flags are omitted.
        let from_cfg =
            resolve_campaign_inputs(None, Some(cfg_str), None, None, None, None, vec![], vec![])
                .expect("config-only resolution");
        assert_eq!(from_cfg.wasm, "from_config.wasm");
        assert_eq!(from_cfg.seed, 42);
        assert_eq!(from_cfg.cases, 50);
        assert_eq!(from_cfg.function.as_deref(), Some("from_cfg_fn"));
        assert_eq!(from_cfg.auth, "correct-auth");
        assert_eq!(from_cfg.expect_success, vec!["cfg_ok".to_string()]);
        assert_eq!(from_cfg.expect_error, vec!["cfg_err:Auth:6".to_string()]);

        // 2. Explicit CLI flags override config values (including explicit --seed 0).
        let overridden = resolve_campaign_inputs(
            None,
            Some(cfg_str),
            Some(0),
            Some(10),
            Some("cli_fn".to_string()),
            Some("wrong-auth".to_string()),
            vec!["cli_ok".to_string()],
            vec!["cli_err:Storage:3".to_string()],
        )
        .expect("cli override resolution");
        assert_eq!(overridden.wasm, "from_config.wasm");
        assert_eq!(overridden.seed, 0);
        assert_eq!(overridden.cases, 10);
        assert_eq!(overridden.function.as_deref(), Some("cli_fn"));
        assert_eq!(overridden.auth, "wrong-auth");
        assert_eq!(overridden.expect_success, vec!["cli_ok".to_string()]);
        assert_eq!(
            overridden.expect_error,
            vec!["cli_err:Storage:3".to_string()]
        );

        // 3. Built-in defaults apply when neither CLI nor config supplies values.
        let defaults = resolve_campaign_inputs(
            Some("cli_only.wasm".to_string()),
            None,
            None,
            None,
            None,
            None,
            vec![],
            vec![],
        )
        .expect("default resolution");
        assert_eq!(defaults.wasm, "cli_only.wasm");
        assert_eq!(defaults.seed, 0);
        assert_eq!(defaults.cases, 100);
        assert_eq!(defaults.function, None);
        assert_eq!(defaults.auth, "no-auth");
    }

    #[test]
    fn wasm_mismatch_and_missing_wasm_errors() {
        let cfg_path = write_temp_config(
            "wasm-mismatch",
            r#"
[campaign]
wasm = "cfg.wasm"
"#,
        );
        let cfg_str = cfg_path.to_str().unwrap();

        // Matching positional + config succeeds.
        let matched = resolve_campaign_inputs(
            Some("cfg.wasm".to_string()),
            Some(cfg_str),
            None,
            None,
            None,
            None,
            vec![],
            vec![],
        )
        .expect("matching wasm should succeed");
        assert_eq!(matched.wasm, "cfg.wasm");

        // Mismatching positional + config fails naming both values.
        let mismatch_err = resolve_campaign_inputs(
            Some("cli.wasm".to_string()),
            Some(cfg_str),
            None,
            None,
            None,
            None,
            vec![],
            vec![],
        )
        .unwrap_err();
        assert!(
            mismatch_err.contains("cli.wasm") && mismatch_err.contains("cfg.wasm"),
            "mismatch error should name both WASM values: {mismatch_err}"
        );

        // Config without wasm and no positional wasm fails naming the config file.
        let no_wasm_cfg = write_temp_config("no-wasm", "[campaign]\ncases = 5\n");
        let no_wasm_str = no_wasm_cfg.to_str().unwrap();
        let missing_wasm_err = resolve_campaign_inputs(
            None,
            Some(no_wasm_str),
            None,
            None,
            None,
            None,
            vec![],
            vec![],
        )
        .unwrap_err();
        assert!(
            missing_wasm_err.contains(no_wasm_str),
            "missing wasm error should name config file: {missing_wasm_err}"
        );
    }
}
