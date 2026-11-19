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

use sdkt_fuzz::{AuthMode, FuzzRunOptions, ReplayOptions};

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
    replay: bool,
) -> Result<FuzzRunOptions, String> {
    let auth_mode = parse_auth(auth).ok_or_else(|| {
        format!("invalid auth mode '{auth}' (expected no-auth, correct-auth, wrong-auth)")
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
        replay,
    })
}

/// Run one campaign and render it. `Ok(true)` = findings produced, `Ok(false)`
/// = clean run; `Err(_)` = usage/input error.
//
/// The argument list mirrors clap's campaign flag surface one-to-one; the
/// engine already bundles these into `FuzzRunOptions` via `options_from`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    wasm: &str,
    seed: u64,
    cases: usize,
    function: Option<String>,
    artifact_dir: Option<String>,
    auth: &str,
    expect_success: Vec<String>,
    expect_error: Vec<String>,
    replay: bool,
    format: &str,
) -> Result<bool, String> {
    let options = options_from(
        wasm,
        seed,
        cases,
        function,
        artifact_dir,
        auth,
        expect_success,
        expect_error,
        replay,
    )?;
    let fmt = parse_format(format)
        .ok_or_else(|| format!("invalid format '{format}' (expected pretty, json, or jsonl)"))?;

    let report = sdkt_fuzz::run_fuzz(&options).map_err(|e| e.to_string())?;
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
