//! `sdkt run <SCENARIO.toml>` — execute a checked-in multi-step contract
//! scenario.
//!
//! A scenario is a linear `[[steps]]` list. Each step is either a read-only
//! `call` or a state-changing `invoke` against a contract (given by `C...` ID
//! or by a workspace alias recorded in `.sdkt-deployments.json` for the
//! selected network profile). Later steps may interpolate earlier steps'
//! outputs with `${steps.<id>.result}` / `${steps.<id>.tx_hash}`, and each step
//! may assert on its recorded output.
//!
//! Execution deliberately goes through the *existing* library paths:
//! [`sdkt_rpc::simulate_transaction`] for `call` steps and
//! [`sdkt_rpc::invoke_contract`] for `invoke` steps. There is no second
//! invocation implementation here.

use crate::{parse_format_str, parse_typed_args, NetworkArgs};
use sdkt_core::deployment::{network_key, DeploymentRecordFile, DEPLOYMENT_RECORD_FILE};
use sdkt_core::OutputFormat;
use sdkt_rpc::SorobanRpcClient;
use sdkt_xdr::builder::InvokeTransactionParams;
use sdkt_xdr::Ed25519Signer;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

/// Placeholder source account used for read-only simulations (identical to the
/// one `sdkt call` uses: nothing is ever signed for it).
const DUMMY_SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

/// Argument types accepted by the shared `parse_typed_args` parser.
const ARG_TYPES: &[&str] = &[
    "u32",
    "i32",
    "u64",
    "i64",
    "timepoint",
    "duration",
    "u128",
    "i128",
    "bool",
    "string",
    "symbol",
    "bytes",
    "address",
];

const EXIT_OK: i32 = 0;
const EXIT_STEP_FAILED: i32 = 1;
const EXIT_USAGE: i32 = 2;

// ---------------------------------------------------------------------------
// Scenario file schema
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ScenarioFile {
    #[serde(default)]
    steps: Vec<StepSpec>,
}

#[derive(Debug, Deserialize)]
struct StepSpec {
    id: String,
    kind: String,
    contract: String,
    function: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    args_json: Vec<String>,
    #[serde(default)]
    identity: Option<String>,
    #[serde(default)]
    no_wait: bool,
    /// `[[steps.assert]]` tables attached to this step.
    #[serde(default, rename = "assert")]
    assertions: Vec<AssertionSpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct AssertionSpec {
    /// JSON pointer (RFC 6901) into the step's recorded output, e.g. `/result`.
    path: String,
    #[serde(default)]
    equals: Option<Value>,
    #[serde(default)]
    exists: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
    Call,
    Invoke,
}

impl StepKind {
    fn as_str(self) -> &'static str {
        match self {
            StepKind::Call => "call",
            StepKind::Invoke => "invoke",
        }
    }

    /// Fields a step of this kind records and therefore can expose through
    /// `${steps.<id>.<field>}`.
    fn produced_fields(self) -> &'static [&'static str] {
        match self {
            StepKind::Call => &["result"],
            StepKind::Invoke => &["tx_hash"],
        }
    }
}

/// A validated step with its contract alias already resolved.
#[derive(Debug)]
struct PreparedStep {
    id: String,
    kind: StepKind,
    contract_id: String,
    function: String,
    args: Vec<String>,
    args_json: Vec<String>,
    identity: String,
    no_wait: bool,
    assertions: Vec<AssertionSpec>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run a scenario file. Returns the process exit code:
/// 0 = every step and assertion passed, 1 = a step or assertion failed,
/// 2 = usage error (unreadable file, malformed TOML, validation failure).
pub(crate) async fn run_scenario(
    scenario_path: &str,
    format: &str,
    dry_run: bool,
    net: &NetworkArgs,
) -> i32 {
    let fmt = parse_format_str(format);

    let raw = match std::fs::read_to_string(scenario_path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("Error: cannot read scenario file '{scenario_path}': {e}");
            return EXIT_USAGE;
        }
    };
    let scenario: ScenarioFile = match toml::from_str(&raw) {
        Ok(scenario) => scenario,
        Err(e) => {
            eprintln!("Error: malformed scenario file '{scenario_path}': {e}");
            return EXIT_USAGE;
        }
    };

    let network_config = match crate::commands::network::resolve_network_config(
        net.rpc_url.clone(),
        net.network_passphrase.clone(),
        net.network_profile.clone(),
    ) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Error: {e}");
            return EXIT_USAGE;
        }
    };
    let network_key = network_key(net.network_profile.as_deref(), &network_config.passphrase);

    let records = match DeploymentRecordFile::read(DEPLOYMENT_RECORD_FILE) {
        Ok(records) => records,
        Err(e) => {
            eprintln!("Error: {e}");
            return EXIT_USAGE;
        }
    };

    let steps = match validate(&scenario, &network_key, &records) {
        Ok(steps) => steps,
        Err(e) => {
            eprintln!("Error: {e}");
            return EXIT_USAGE;
        }
    };

    let client = SorobanRpcClient::from_config(&network_config);
    execute_steps(
        scenario_path,
        &steps,
        dry_run,
        fmt,
        &client,
        &network_config,
        net,
    )
    .await
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate(
    scenario: &ScenarioFile,
    network: &str,
    records: &DeploymentRecordFile,
) -> Result<Vec<PreparedStep>, String> {
    if scenario.steps.is_empty() {
        return Err("scenario has no [[steps]] entries".into());
    }

    let mut seen: HashMap<&str, StepKind> = HashMap::new();
    let mut prepared = Vec::with_capacity(scenario.steps.len());

    for spec in &scenario.steps {
        if !is_valid_step_id(&spec.id) {
            return Err(format!(
                "invalid step id '{}': ids must match [A-Za-z0-9_-]+",
                spec.id
            ));
        }
        if seen.contains_key(spec.id.as_str()) {
            return Err(format!("duplicate step id '{}'", spec.id));
        }

        let kind = match spec.kind.as_str() {
            "call" => StepKind::Call,
            "invoke" => StepKind::Invoke,
            other => {
                return Err(format!(
                    "step '{}': unknown kind '{other}' (expected 'call' or 'invoke')",
                    spec.id
                ))
            }
        };
        if spec.function.trim().is_empty() {
            return Err(format!("step '{}': function must not be empty", spec.id));
        }

        let contract_id = resolve_contract(&spec.contract, network, records)
            .map_err(|e| format!("step '{}': {e}", spec.id))?;

        for arg in &spec.args {
            validate_arg(arg).map_err(|e| format!("step '{}': {e}", spec.id))?;
            for reference in references_in(arg) {
                validate_reference(&spec.id, &reference, &seen, kind)?;
            }
        }
        for arg in &spec.args_json {
            let refs = references_in(arg);
            for reference in &refs {
                validate_reference(&spec.id, reference, &seen, kind)?;
            }
            if refs.is_empty() && serde_json::from_str::<Value>(arg).is_err() {
                return Err(format!(
                    "step '{}': args_json is not valid JSON: {arg}",
                    spec.id
                ));
            }
        }

        for assertion in &spec.assertions {
            if !assertion.path.starts_with('/') {
                return Err(format!(
                    "step '{}': assertion path '{}' must be a JSON pointer starting with '/'",
                    spec.id, assertion.path
                ));
            }
            match (&assertion.equals, assertion.exists) {
                (Some(_), None) | (None, Some(_)) => {}
                (Some(_), Some(_)) => {
                    return Err(format!(
                        "step '{}': assertion '{}' cannot set both equals and exists",
                        spec.id, assertion.path
                    ))
                }
                (None, None) => {
                    return Err(format!(
                        "step '{}': assertion '{}' must set equals or exists",
                        spec.id, assertion.path
                    ))
                }
            }
        }

        seen.insert(spec.id.as_str(), kind);
        prepared.push(PreparedStep {
            id: spec.id.clone(),
            kind,
            contract_id,
            function: spec.function.clone(),
            args: spec.args.clone(),
            args_json: spec.args_json.clone(),
            identity: spec
                .identity
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            no_wait: spec.no_wait,
            assertions: spec.assertions.clone(),
        });
    }

    Ok(prepared)
}

fn is_valid_step_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A contract is either a `C...` StrKey contract ID or a workspace alias.
fn resolve_contract(
    raw: &str,
    network: &str,
    records: &DeploymentRecordFile,
) -> Result<String, String> {
    if is_contract_id(raw) {
        return Ok(raw.to_string());
    }
    match records.record_for(network, raw) {
        Some(record) => Ok(record.contract_id.clone()),
        None => Err(format!(
            "contract alias '{raw}' is not recorded in {DEPLOYMENT_RECORD_FILE} for network '{network}'"
        )),
    }
}

fn is_contract_id(raw: &str) -> bool {
    raw.len() == 56
        && raw.starts_with('C')
        && raw
            .bytes()
            .all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b))
}

fn validate_arg(arg: &str) -> Result<(), String> {
    let Some((kind, _value)) = arg.split_once(':') else {
        return Err(format!(
            "invalid arg format '{arg}'. Use TYPE:VALUE (e.g. u32:100, address:G...)"
        ));
    };
    if !ARG_TYPES.contains(&kind.to_lowercase().as_str()) {
        return Err(format!(
            "unknown arg type '{kind}'. Use {}",
            ARG_TYPES.join("|")
        ));
    }
    // Arguments without interpolation can be fully validated now; ones that
    // reference an earlier step are validated again after substitution.
    if references_in(arg).is_empty() {
        parse_typed_args(&[arg.to_string()], true)?;
    }
    Ok(())
}

fn validate_reference(
    step_id: &str,
    reference: &Reference,
    seen: &HashMap<&str, StepKind>,
    _kind: StepKind,
) -> Result<(), String> {
    let Some(kind) = seen.get(reference.step.as_str()) else {
        return Err(format!(
            "step '{step_id}': '${{{}}}' references unknown step '{}'",
            reference.raw, reference.step
        ));
    };
    if !kind.produced_fields().contains(&reference.field.as_str()) {
        return Err(format!(
            "step '{step_id}': '${{{}}}' references field '{}', which a '{}' step does not produce",
            reference.raw,
            reference.field,
            kind.as_str()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Value passing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Reference {
    /// The full token, e.g. `steps.balance.result`.
    raw: String,
    /// The referenced step id.
    step: String,
    /// `result` or `tx_hash`.
    field: String,
}

/// Find every `${steps.<id>.<field>}` token in `input`.
fn references_in(input: &str) -> Vec<Reference> {
    let mut found = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            break;
        };
        let inner = &after[..end];
        let mut parts = inner.split('.');
        if parts.next() == Some("steps") {
            if let (Some(step), Some(field), None) = (parts.next(), parts.next(), parts.next()) {
                if !step.is_empty() && !field.is_empty() {
                    found.push(Reference {
                        raw: inner.to_string(),
                        step: step.to_string(),
                        field: field.to_string(),
                    });
                }
            }
        }
        rest = &after[end + 1..];
    }
    found
}

/// Substitute every reference in `input` using the outputs recorded so far.
fn substitute(input: &str, outputs: &HashMap<String, Value>) -> Result<String, String> {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!("unterminated reference in '{input}'"));
        };
        let inner = &after[..end];
        let mut parts = inner.split('.');
        if parts.next() != Some("steps") {
            return Err(format!("unknown reference '${{{inner}}}'"));
        }
        let (Some(step), Some(field), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(format!(
                "malformed reference '${{{inner}}}' (expected steps.<id>.result|tx_hash)"
            ));
        };
        let Some(output) = outputs.get(step) else {
            return Err(format!(
                "reference '${{{inner}}}' resolves before step '{step}' ran"
            ));
        };
        let Some(value) = output.get(field) else {
            return Err(format!(
                "reference '${{{inner}}}': step '{step}' produced no '{field}'"
            ));
        };
        result.push_str(&value_to_argument(value).ok_or_else(|| {
            format!(
                "reference '${{{inner}}}' resolved to null, which cannot be used as an argument"
            )
        })?);
        rest = &after[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

/// Render a recorded JSON value as an argument fragment.
fn value_to_argument(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(_) | Value::Object(_) => Some(value.to_string()),
    }
}

/// Render a 128-bit integer as a JSON number when it fits in `i64`, otherwise
/// as a lossless decimal string.
fn wide_int_to_json(rendered: &str) -> Value {
    match rendered.parse::<i128>() {
        Ok(n) => match i64::try_from(n) {
            Ok(small) => json!(small),
            Err(_) => json!(rendered),
        },
        Err(_) => json!(rendered),
    }
}

/// Convert a simulated `ScVal` result into JSON for reporting, assertions and
/// interpolation.
fn scval_to_json(value: &stellar_xdr::ScVal) -> Value {
    use stellar_xdr::ScVal;
    match value {
        ScVal::Bool(b) => json!(b),
        ScVal::Void => Value::Null,
        ScVal::U32(n) => json!(n),
        ScVal::I32(n) => json!(n),
        ScVal::U64(n) => json!(n),
        ScVal::I64(n) => json!(n),
        ScVal::Timepoint(t) => json!(t.0),
        ScVal::Duration(d) => json!(d.0),
        ScVal::U128(p) => wide_int_to_json(&p.to_string()),
        ScVal::I128(p) => wide_int_to_json(&p.to_string()),
        ScVal::U256(p) => json!(p.to_string()),
        ScVal::I256(p) => json!(p.to_string()),
        ScVal::String(s) => json!(s.to_utf8_string_lossy()),
        ScVal::Symbol(s) => json!(s.to_utf8_string_lossy()),
        ScVal::Bytes(b) => json!(hex::encode(b.as_slice())),
        ScVal::Address(a) => json!(a.to_string()),
        ScVal::Vec(items) => {
            let items = items.as_ref().map(|v| v.as_slice()).unwrap_or_default();
            Value::Array(items.iter().map(scval_to_json).collect())
        }
        ScVal::Map(entries) => {
            let entries = entries.as_ref().map(|m| m.as_slice()).unwrap_or_default();
            let string_keys = entries.iter().all(|e| {
                matches!(
                    &e.key,
                    ScVal::Symbol(_) | ScVal::String(_) | ScVal::U32(_) | ScVal::I32(_)
                )
            });
            if string_keys {
                let mut map = serde_json::Map::new();
                for entry in entries {
                    map.insert(
                        value_to_argument(&scval_to_json(&entry.key)).unwrap_or_default(),
                        scval_to_json(&entry.val),
                    );
                }
                Value::Object(map)
            } else {
                Value::Array(
                    entries
                        .iter()
                        .map(|e| json!({ "key": scval_to_json(&e.key), "value": scval_to_json(&e.val) }))
                        .collect(),
                )
            }
        }
        other => json!(sdkt_xdr::render_scval(other)),
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

struct StepOutcome {
    id: String,
    kind: StepKind,
    status: &'static str,
    output: Value,
    error: Option<String>,
    assertion_results: Vec<Value>,
}

async fn execute_steps(
    scenario_path: &str,
    steps: &[PreparedStep],
    dry_run: bool,
    fmt: OutputFormat,
    client: &SorobanRpcClient,
    network_config: &sdkt_core::NetworkConfig,
    net: &NetworkArgs,
) -> i32 {
    let mut outputs: HashMap<String, Value> = HashMap::new();
    let mut outcomes: Vec<StepOutcome> = Vec::new();

    for step in steps {
        let args = match resolve_step_args(step, &outputs) {
            Ok(args) => args,
            Err(e) => {
                outcomes.push(StepOutcome {
                    id: step.id.clone(),
                    kind: step.kind,
                    status: "failed",
                    output: Value::Null,
                    error: Some(e),
                    assertion_results: Vec::new(),
                });
                return finish(scenario_path, dry_run, fmt, outcomes, EXIT_STEP_FAILED);
            }
        };

        let executed = match step.kind {
            StepKind::Call => execute_call_step(client, step, &args).await,
            StepKind::Invoke if dry_run => simulate_invoke_step(client, step, &args).await,
            StepKind::Invoke => execute_invoke_step(client, step, &args, network_config, net).await,
        };

        let (output, failure) = match executed {
            Ok(output) => (output, None),
            Err(e) => (Value::Null, Some(e)),
        };

        let assertion_results = if failure.is_none() && !dry_run {
            evaluate_assertions(&step.assertions, &output)
        } else {
            step.assertions
                .iter()
                .map(|a| {
                    json!({
                        "path": a.path,
                        "status": "skipped",
                    })
                })
                .collect()
        };

        let assertion_failure = assertion_results
            .iter()
            .find(|r| r["status"] == "failed")
            .map(|r| {
                format!(
                    "assertion {} failed: {}",
                    r["path"].as_str().unwrap_or(""),
                    r["message"].as_str().unwrap_or("value did not match")
                )
            });

        let status = if failure.is_none() && assertion_failure.is_none() {
            if dry_run {
                "simulated"
            } else {
                "passed"
            }
        } else {
            "failed"
        };

        outputs.insert(step.id.clone(), output.clone());
        outcomes.push(StepOutcome {
            id: step.id.clone(),
            kind: step.kind,
            status,
            output,
            error: failure.or(assertion_failure),
            assertion_results,
        });

        if status == "failed" {
            return finish(scenario_path, dry_run, fmt, outcomes, EXIT_STEP_FAILED);
        }
    }

    print_summary(scenario_path, dry_run, fmt, &outcomes);
    EXIT_OK
}

fn finish(
    scenario_path: &str,
    dry_run: bool,
    fmt: OutputFormat,
    outcomes: Vec<StepOutcome>,
    code: i32,
) -> i32 {
    if let Some(failed) = outcomes.iter().find(|o| o.status == "failed") {
        eprintln!(
            "Error: step '{}' failed: {}",
            failed.id,
            failed.error.as_deref().unwrap_or("unknown error")
        );
    }
    print_summary(scenario_path, dry_run, fmt, &outcomes);
    code
}

fn resolve_step_args(
    step: &PreparedStep,
    outputs: &HashMap<String, Value>,
) -> Result<StepArgs, String> {
    let args = step
        .args
        .iter()
        .map(|a| substitute(a, outputs))
        .collect::<Result<Vec<_>, _>>()?;
    let args_json = step
        .args_json
        .iter()
        .map(|a| substitute(a, outputs))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StepArgs {
        json: args,
        args_json,
    })
}

struct StepArgs {
    json: Vec<String>,
    args_json: Vec<String>,
}

impl StepArgs {
    fn parse(&self) -> Result<Vec<String>, String> {
        let mut parsed = parse_typed_args(&self.json, true)?;
        for raw in &self.args_json {
            parsed.extend(sdkt_xdr::json_args_to_base64(raw).map_err(|e| e.to_string())?);
        }
        Ok(parsed)
    }
}

async fn execute_call_step(
    client: &SorobanRpcClient,
    step: &PreparedStep,
    args: &StepArgs,
) -> Result<Value, String> {
    let parsed_args = args.parse()?;
    let params = InvokeTransactionParams {
        source_account: DUMMY_SOURCE.into(),
        sequence: 0,
        fee: 0,
        contract_id: step.contract_id.clone(),
        function: step.function.clone(),
        args: parsed_args,
        memo: None,
    };
    let envelope = sdkt_xdr::builder::build_invoke_transaction(&params)
        .map_err(|e| format!("failed to build simulation envelope: {e}"))?;

    let response = sdkt_rpc::simulate_transaction(client, &envelope)
        .await
        .map_err(|e| format!("RPC simulation failed: {e}"))?;
    if let Some(err) = &response.error {
        return Err(format!("simulation error: {err}"));
    }

    let result_raw = response
        .results
        .first()
        .map(|r| r.xdr.clone())
        .unwrap_or_default();
    let result = if result_raw.is_empty() {
        Value::Null
    } else {
        match sdkt_xdr::scval_from_base64(&result_raw) {
            Some(scval) => scval_to_json(&scval),
            None => json!(result_raw),
        }
    };
    Ok(json!({
        "result": result,
        "result_xdr": result_raw,
        "events": response.events,
    }))
}

async fn execute_invoke_step(
    client: &SorobanRpcClient,
    step: &PreparedStep,
    args: &StepArgs,
    network_config: &sdkt_core::NetworkConfig,
    net: &NetworkArgs,
) -> Result<Value, String> {
    let explicit = crate::commands::network::network_is_explicit(
        &net.rpc_url,
        &net.network_passphrase,
        &net.network_profile,
    );
    sdkt_core::guard_mutating_network(network_config, explicit).map_err(|e| e.to_string())?;

    let network = match network_config.passphrase.as_str() {
        "Public Global Stellar Network ; September 2015" => sdkt_xdr::Network::Mainnet,
        "Test SDF Future Network ; October 2022" => sdkt_xdr::Network::Futurenet,
        other => sdkt_xdr::Network::Custom(other.to_string()),
    };

    let identity_store = sdkt_storage::IdentityStore::new()
        .map_err(|e| format!("failed to access identity store: {e}"))?;
    let identity = identity_store
        .resolve_signing_identity(&step.identity)
        .map_err(|e| format!("failed to resolve identity '{}': {e}", step.identity))?;
    let signing_key = identity_store
        .load_signing_key(&identity.name)
        .map_err(|e| format!("failed to load signing key for '{}': {e}", identity.name))?;
    let signer = Ed25519Signer::from_seed(&signing_key.to_bytes());

    let parsed_args = args.parse()?;
    let params = InvokeTransactionParams {
        source_account: identity.public_key.clone(),
        sequence: 0,
        fee: 0,
        contract_id: step.contract_id.clone(),
        function: step.function.clone(),
        args: parsed_args,
        memo: None,
    };
    let poll = sdkt_rpc::PollConfig {
        timeout: Duration::from_secs(60),
        interval: Duration::from_secs(1),
    };

    let result = sdkt_rpc::invoke_contract(client, &params, &signer, network, &poll, !step.no_wait)
        .await
        .map_err(|e| format!("invoke failed: {e}"))?;

    let success = result.status == "SUCCESS" || (step.no_wait && result.status == "PENDING");
    if !success {
        return Err(format!(
            "transaction {} ended with status {}{}",
            result.hash,
            result.status,
            result
                .error_code
                .as_ref()
                .map(|c| format!(" ({c})"))
                .unwrap_or_default()
        ));
    }

    Ok(json!({
        "tx_hash": result.hash,
        "status": result.status,
        "fee": result.fee,
        "result_xdr": result.result_xdr,
        "events": result.events,
    }))
}

/// Dry-run for `invoke`: run the *simulation* only. Never builds a final
/// envelope, never signs, never submits.
async fn simulate_invoke_step(
    client: &SorobanRpcClient,
    step: &PreparedStep,
    args: &StepArgs,
) -> Result<Value, String> {
    let parsed_args = args.parse()?;
    let params = InvokeTransactionParams {
        source_account: DUMMY_SOURCE.into(),
        sequence: 0,
        fee: 0,
        contract_id: step.contract_id.clone(),
        function: step.function.clone(),
        args: parsed_args,
        memo: None,
    };
    let simulated = sdkt_rpc::simulate_invoke(client, &params)
        .await
        .map_err(|e| format!("simulation failed: {e}"))?;
    Ok(json!({
        "status": "SIMULATED",
        "fee": simulated.total_fee,
        "submitted": false,
    }))
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

fn evaluate_assertions(assertions: &[AssertionSpec], output: &Value) -> Vec<Value> {
    assertions
        .iter()
        .map(|assertion| {
            let actual = output.pointer(&assertion.path);
            if let Some(expected_exists) = assertion.exists {
                let exists = actual.is_some();
                return if exists == expected_exists {
                    json!({ "path": assertion.path, "kind": "exists", "expected": expected_exists, "status": "passed" })
                } else {
                    json!({
                        "path": assertion.path,
                        "kind": "exists",
                        "expected": expected_exists,
                        "actual": exists,
                        "status": "failed",
                        "message": format!("expected the pointer to {} but it {}", if expected_exists { "exist" } else { "not exist" }, if exists { "does" } else { "does not" }),
                    })
                };
            }

            let expected = assertion.equals.clone().unwrap_or(Value::Null);
            match actual {
                Some(actual) if *actual == expected => json!({
                    "path": assertion.path,
                    "kind": "equals",
                    "expected": expected,
                    "actual": actual,
                    "status": "passed",
                }),
                Some(actual) => json!({
                    "path": assertion.path,
                    "kind": "equals",
                    "expected": expected,
                    "actual": actual,
                    "status": "failed",
                    "message": format!("expected {expected}, found {actual}"),
                }),
                None => json!({
                    "path": assertion.path,
                    "kind": "equals",
                    "expected": expected,
                    "status": "failed",
                    "message": format!("pointer '{}' is not present in the step output", assertion.path),
                }),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

fn print_summary(scenario_path: &str, dry_run: bool, fmt: OutputFormat, outcomes: &[StepOutcome]) {
    let status = if outcomes.iter().any(|o| o.status == "failed") {
        "failed"
    } else if dry_run {
        "simulated"
    } else {
        "passed"
    };

    if fmt == OutputFormat::Json {
        let steps: Vec<Value> = outcomes
            .iter()
            .map(|outcome| {
                let mut entry = json!({
                    "id": outcome.id,
                    "kind": outcome.kind.as_str(),
                    "status": outcome.status,
                    "assertions": outcome.assertion_results,
                });
                if let Some(hash) = outcome.output.get("tx_hash") {
                    entry["tx_hash"] = hash.clone();
                }
                if let Some(result) = outcome.output.get("result") {
                    entry["result"] = result.clone();
                }
                entry["output"] = outcome.output.clone();
                if let Some(error) = &outcome.error {
                    entry["error"] = json!(error);
                }
                entry
            })
            .collect();
        let summary = json!({
            "scenario": scenario_path,
            "dry_run": dry_run,
            "status": status,
            "steps": steps,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".into())
        );
        return;
    }

    println!("Scenario: {scenario_path}");
    if dry_run {
        println!("Mode:     dry-run (nothing signed or submitted)");
    }
    println!();
    for outcome in outcomes {
        let marker = match outcome.status {
            "failed" => "FAILED",
            "simulated" => "SIMULATED",
            _ => "PASSED",
        };
        println!("Step {} ({}) — {marker}", outcome.id, outcome.kind.as_str());
        if let Some(result) = outcome.output.get("result") {
            println!("  Result:  {}", render_value(result));
        }
        if let Some(hash) = outcome.output.get("tx_hash").and_then(Value::as_str) {
            println!("  Tx hash: {hash}");
        }
        for assertion in &outcome.assertion_results {
            let status = assertion["status"].as_str().unwrap_or("");
            let path = assertion["path"].as_str().unwrap_or("");
            if status == "skipped" {
                println!("  Assert  {path}: skipped (dry-run)");
            } else {
                println!("  Assert  {path}: {status}");
            }
            if let Some(message) = assertion["message"].as_str() {
                println!("          {message}");
            }
        }
        if let Some(error) = &outcome.error {
            println!("  Error:   {error}");
        }
        println!();
    }
    println!("Result: {status} ({} step(s))", outcomes.len());
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn records_with(alias: &str, contract_id: &str) -> DeploymentRecordFile {
        let mut records = DeploymentRecordFile::default();
        records.set_record(
            "testnet",
            alias,
            sdkt_core::deployment::DeploymentRecord {
                contract_id: contract_id.to_string(),
                wasm_hash: "00".into(),
                network: "testnet".into(),
                timestamp: 0,
                salt: None,
            },
        );
        records
    }

    const CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

    fn scenario(toml_src: &str) -> ScenarioFile {
        toml::from_str(toml_src).expect("scenario should parse")
    }

    #[test]
    fn value_passing_substitutes_call_result_and_invoke_hash() {
        let mut outputs = HashMap::new();
        outputs.insert("balance".to_string(), json!({ "result": 500 }));
        outputs.insert("transfer".to_string(), json!({ "tx_hash": "abc123" }));

        assert_eq!(
            substitute("i128:${steps.balance.result}", &outputs).unwrap(),
            "i128:500"
        );
        assert_eq!(
            substitute("string:${steps.transfer.tx_hash}", &outputs).unwrap(),
            "string:abc123"
        );
        assert_eq!(
            substitute("address:GAAA...${steps.balance.result}", &outputs).unwrap(),
            "address:GAAA...500"
        );
    }

    #[test]
    fn unknown_step_reference_is_a_validation_error_naming_the_step() {
        let parsed = scenario(
            r#"
[[steps]]
id = "transfer"
kind = "invoke"
contract = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC"
function = "transfer"
args = ["i128:${steps.nope.result}"]
"#,
        );
        let err = validate(&parsed, "testnet", &DeploymentRecordFile::default()).unwrap_err();
        assert!(
            err.contains("step 'transfer'") && err.contains("unknown step 'nope'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unknown_alias_is_a_validation_error_naming_the_step() {
        let parsed = scenario(
            r#"
[[steps]]
id = "balance"
kind = "call"
contract = "token"
function = "balance"
"#,
        );
        let err = validate(&parsed, "testnet", &DeploymentRecordFile::default()).unwrap_err();
        assert!(
            err.contains("step 'balance'") && err.contains("alias 'token'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn duplicate_step_ids_are_rejected() {
        let parsed = scenario(
            r#"
[[steps]]
id = "balance"
kind = "call"
contract = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC"
function = "balance"

[[steps]]
id = "balance"
kind = "call"
contract = "CAE3U7JKESRWPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC"
function = "balance"
"#,
        );
        let err = validate(&parsed, "testnet", &DeploymentRecordFile::default()).unwrap_err();
        assert!(err.contains("duplicate step id 'balance'"), "{err}");
    }

    #[test]
    fn alias_resolves_through_deployment_record() {
        let records = records_with("token", CONTRACT);
        let parsed = scenario(
            r#"
[[steps]]
id = "balance"
kind = "call"
contract = "token"
function = "balance"
"#,
        );
        let steps = validate(&parsed, "testnet", &records).unwrap();
        assert_eq!(steps[0].contract_id, CONTRACT);
    }

    #[test]
    fn assertions_evaluate_equals_and_exists() {
        let output = json!({ "result": 500, "tx_hash": "abc" });
        let assertions = vec![
            AssertionSpec {
                path: "/result".into(),
                equals: Some(json!(500)),
                exists: None,
            },
            AssertionSpec {
                path: "/tx_hash".into(),
                equals: None,
                exists: Some(true),
            },
        ];
        let results = evaluate_assertions(&assertions, &output);
        assert!(results.iter().all(|r| r["status"] == "passed"));
    }

    #[test]
    fn failed_equals_assertion_reports_expected_and_actual() {
        let output = json!({ "result": 500 });
        let assertions = vec![AssertionSpec {
            path: "/result".into(),
            equals: Some(json!(999)),
            exists: None,
        }];
        let results = evaluate_assertions(&assertions, &output);
        assert_eq!(results[0]["status"], "failed");
        assert!(results[0]["message"]
            .as_str()
            .unwrap()
            .contains("expected 999"));
    }
}
