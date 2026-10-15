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
    let finding =
        sdkt_fuzz::load_artifact(std::path::Path::new(artifact)).map_err(|e| e.to_string())?;
    let outcome = sdkt_fuzz::run_replay(&options).map_err(|e| e.to_string())?;
    print_replay(&outcome, &finding, &fmt);
    Ok(outcome.is_reproduced())
}

/// Render a campaign report.
fn print_report(report: &sdkt_fuzz::FuzzReport, fmt: &FuzzFormat) {
    match fmt {
        FuzzFormat::Json => {
            let json = campaign_json_value(report)
                .and_then(|value| serde_json::to_string(&value).map_err(|error| error.to_string()))
                .unwrap_or_else(|e| {
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
fn print_replay(
    outcome: &sdkt_fuzz::ReplayOutcome,
    finding: &sdkt_fuzz::FindingArtifact,
    fmt: &FuzzFormat,
) {
    match fmt {
        FuzzFormat::Pretty => {
            println!("{}", sdkt_fuzz::replay_line(outcome));
            for reduction in &finding.minimization.reductions {
                println!(
                    "  reduction kind={} complexity_after={} detail={}",
                    reduction.kind, reduction.complexity_after, reduction.detail
                );
            }
        }
        FuzzFormat::Json | FuzzFormat::Jsonl => {
            let mut json = replay_json_value(outcome);
            json["minimization"] = minimization_snapshot_json(&finding.minimization);
            println!(
                "{}",
                serde_json::to_string(&json).expect("replay view serializes")
            );
        }
    }
}

#[cfg(test)]
mod minimization_json_tests {
    use super::*;
    use sdkt_fuzz::artifact::MinimizationSnapshot;

    #[test]
    fn replay_json_keeps_reduction_trace_empty_when_no_reductions_were_accepted() {
        let minimization = MinimizationSnapshot {
            attempted: true,
            strategy: "deterministic".into(),
            original_complexity: 3,
            minimized_complexity: 3,
            preserved: true,
            reductions: vec![],
        };
        let value = minimization_snapshot_json(&minimization);
        assert_eq!(value["reductions"], serde_json::json!([]));
    }

    #[test]
    fn campaign_json_includes_each_reduction_field() {
        let value = reduction_json("arg", "argument 0 -> 0", 2);
        assert_eq!(value["kind"], "arg");
        assert_eq!(value["detail"], "argument 0 -> 0");
        assert_eq!(value["complexity_after"], 2);
    }

    #[test]
    fn replay_json_includes_artifact_reductions() {
        let minimization = MinimizationSnapshot {
            attempted: true,
            strategy: "deterministic".into(),
            original_complexity: 5,
            minimized_complexity: 2,
            preserved: true,
            reductions: vec![sdkt_fuzz::artifact::ReductionSnapshot {
                kind: "step".into(),
                detail: "remove trailing step".into(),
                complexity_after: 2,
            }],
        };
        let value = minimization_snapshot_json(&minimization);
        assert_eq!(value["reductions"][0]["kind"], "step");
        assert_eq!(value["reductions"][0]["detail"], "remove trailing step");
        assert_eq!(value["reductions"][0]["complexity_after"], 2);
    }
}

fn minimization_snapshot_json(
    minimization: &sdkt_fuzz::artifact::MinimizationSnapshot,
) -> serde_json::Value {
    serde_json::json!({
        "attempted": minimization.attempted,
        "strategy": minimization.strategy,
        "original_complexity": minimization.original_complexity,
        "minimized_complexity": minimization.minimized_complexity,
        "preserved": minimization.preserved,
        "reductions": minimization.reductions.iter().map(|reduction| {
            reduction_json(&reduction.kind, &reduction.detail, reduction.complexity_after)
        }).collect::<Vec<_>>(),
    })
}

fn reduction_json(kind: &str, detail: &str, complexity_after: usize) -> serde_json::Value {
    serde_json::json!({
        "kind": kind,
        "detail": detail,
        "complexity_after": complexity_after,
    })
}

fn campaign_json_value(report: &sdkt_fuzz::FuzzReport) -> Result<serde_json::Value, String> {
    let mut value = serde_json::to_value(&report.evidence).map_err(|e| e.to_string())?;
    if let Some(findings) = value
        .get_mut("findings")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (json_finding, finding) in findings.iter_mut().zip(&report.result.findings) {
            json_finding["minimization"] = serde_json::json!({
                "attempted": finding.minimization.attempted,
                "strategy": finding.minimization.strategy,
                "original_complexity": finding.minimization.original_complexity,
                "minimized_complexity": finding.minimization.minimized_complexity,
                "preserved": finding.minimization.preserved,
                "reductions": finding.minimization.reductions.iter().map(|reduction| {
                    reduction_json(&reduction.kind, &reduction.detail, reduction.complexity_after)
                }).collect::<Vec<_>>(),
            });
        }
    }
    Ok(value)
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
