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

use sdkt_fuzz::{AuthMode, FuzzRunOptions, ReasonCode, ReplayOptions};
use sdkt_wasm::ContractType;
use std::collections::BTreeMap;

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

/// Print the oracle vocabulary and mutations available to the generator.
pub fn run_list(format: &str) -> Result<(), String> {
    let fmt = parse_format(format)
        .ok_or_else(|| format!("invalid format '{format}' (expected pretty or json)"))?;
    if matches!(fmt, FuzzFormat::Jsonl) {
        return Err(format!(
            "invalid format '{format}' (expected pretty or json)"
        ));
    }

    let reason_codes = [
        (
            ReasonCode::ReturnMismatch,
            "Declared return value did not match.",
        ),
        (
            ReasonCode::StateMismatch,
            "Declared state changes did not match.",
        ),
        (ReasonCode::EventMismatch, "Declared events did not match."),
        (
            ReasonCode::AuthorizationBypass,
            "A protected operation succeeded without authorization.",
        ),
        (
            ReasonCode::UnexpectedError,
            "A call failed despite a declared success expectation.",
        ),
        (
            ReasonCode::ResourceLimit,
            "A call exceeded the declared resource limit.",
        ),
    ];
    let mut operators: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for (contract_type, label) in supported_contract_types() {
        for operator in sdkt_fuzz::mutation::operators_for(&contract_type) {
            operators.entry(operator.name()).or_default().push(label);
        }
    }

    let value = serde_json::json!({
        "reason_codes": reason_codes.iter().map(|(code, meaning)| {
            serde_json::json!({ "name": code.name(), "meaning": meaning })
        }).collect::<Vec<_>>(),
        "mutation_operators": operators.iter().map(|(name, types)| {
            serde_json::json!({ "name": name, "types": types })
        }).collect::<Vec<_>>(),
    });

    match fmt {
        FuzzFormat::Pretty => print_list_pretty(&value),
        FuzzFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("list serializes")
        ),
        FuzzFormat::Jsonl => unreachable!(),
    }
    Ok(())
}

fn supported_contract_types() -> Vec<(ContractType, &'static str)> {
    let mut types = [
        "bool",
        "u32",
        "i32",
        "u64",
        "i64",
        "u128",
        "i128",
        "string",
        "bytes",
        "address",
        "muxed_address",
    ]
    .into_iter()
    .map(|name| (contract_type(name, "primitive", Vec::new()), name))
    .collect::<Vec<_>>();

    let u32_type = contract_type("u32", "primitive", Vec::new());
    types.extend([
        (
            contract_type("vec<u32>", "compound", vec![u32_type.clone()]),
            "vec<T>",
        ),
        (
            contract_type("option<u32>", "compound", vec![u32_type.clone()]),
            "option<T>",
        ),
        (
            contract_type("map<u32,u32>", "compound", vec![u32_type.clone(), u32_type]),
            "map<K, V>",
        ),
    ]);
    types
}

fn contract_type(name: &str, kind: &str, type_args: Vec<ContractType>) -> ContractType {
    ContractType {
        name: name.to_string(),
        kind: kind.to_string(),
        doc: String::new(),
        members: Vec::new(),
        type_args,
        bytes_n: None,
    }
}

fn print_list_pretty(value: &serde_json::Value) {
    println!("Oracle reason codes");
    println!("{:<24} Meaning", "CODE");
    for code in value["reason_codes"].as_array().expect("reason code array") {
        println!(
            "{:<24} {}",
            code["name"].as_str().unwrap(),
            code["meaning"].as_str().unwrap()
        );
    }
    println!();
    println!("Mutation operators");
    println!("{:<24} Contract types", "OPERATOR");
    for operator in value["mutation_operators"]
        .as_array()
        .expect("operator array")
    {
        let types = operator["types"]
            .as_array()
            .expect("operator type array")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        println!("{:<24} {}", operator["name"].as_str().unwrap(), types);
    }
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
