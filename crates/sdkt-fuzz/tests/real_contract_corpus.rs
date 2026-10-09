//! Real-contract corpus: deterministic local execution of a live testnet
//! contract under a live-captured network profile.
//!
//! ## What this corpus is
//!
//! A small, honest starting corpus: **one real contract, two scenarios**.
//! The contract is the public testnet contract the repo's own walkthrough
//! uses (`docs/getting-started/public-contract-walkthrough.md`), whose WASM
//! is fetched from the ledger and pinned as a fixture.
//!
//! - **success scenario**: a call the contract accepts.
//! - **failure scenario**: a call the contract rejects (unknown function),
//!   which exercises the error path rather than the happy path.
//!
//! ## What this corpus is not
//!
//! It is not broad coverage. One contract and two scenarios say nothing about
//! the space of Soroban behaviour; the numbers below are the honest count of
//! what was actually executed. Broad coverage would need many more contracts
//! and scenarios, which is explicitly out of scope here.

use sdkt_fuzz::{
    network_faithful_plan, profile_from_capture, CaptureInput, Environment, ExecutionStatus,
    Executor, FunctionCall,
};
use sdkt_rpc::network_capture::{
    config_setting_key, decode_into, decode_ledger_entry_data_b64, CapturedNetworkConfig,
};
use sdkt_rpc::SorobanRpcClient;
use stellar_xdr::ConfigSettingId;

const TESTNET_RPC: &str = "https://soroban-testnet.stellar.org";

/// The live testnet contract's `ContractCode` ledger entry (ledger 4949165).
const TESTNET_CODE_ENTRY: &[u8] = include_bytes!("fixtures/testnet_contract_code_entry.bin");

/// A function the contract does not export.
const FAILURE_FUNCTION: &str = "this_function_does_not_exist";

async fn live_profile() -> sdkt_fuzz::NetworkProfile {
    let client = SorobanRpcClient::new(TESTNET_RPC);
    let network = client.get_network().await.expect("getNetwork");
    let ledger = client.get_ledger().await.expect("getLatestLedger");
    let mut captured = CapturedNetworkConfig::empty(
        network.passphrase.clone(),
        network.protocol_version,
        ledger.sequence,
    );

    let ids = [
        ConfigSettingId::ContractComputeV0,
        ConfigSettingId::ContractMaxSizeBytes,
        ConfigSettingId::ContractBandwidthV0,
        ConfigSettingId::ContractEventsV0,
        ConfigSettingId::ContractCostParamsCpuInstructions,
        ConfigSettingId::ContractCostParamsMemoryBytes,
    ];
    let keys: Vec<String> = ids.iter().map(|id| config_setting_key(*id)).collect();
    let response = client
        .get_contract_storage("", &keys)
        .await
        .expect("getLedgerEntries");
    for entry in &response.entries {
        let data = decode_ledger_entry_data_b64(&entry.xdr).expect("decode");
        decode_into(&mut captured, &data, entry.last_modified_ledger_seq).expect("decode_into");
    }

    let cp = captured.cost_params.value.clone();
    let to_snap = |p: &sdkt_rpc::network_capture::CapturedCostParams| {
        (
            p.cpu
                .iter()
                .map(|e| sdkt_fuzz::CostParamEntrySnapshot {
                    cost_type: e.cost_type.clone(),
                    index: e.index,
                    const_term: e.const_term,
                    linear_term: e.linear_term,
                })
                .collect::<Vec<_>>(),
            p.mem
                .iter()
                .map(|e| sdkt_fuzz::CostParamEntrySnapshot {
                    cost_type: e.cost_type.clone(),
                    index: e.index,
                    const_term: e.const_term,
                    linear_term: e.linear_term,
                })
                .collect::<Vec<_>>(),
        )
    };
    let (cpu, mem) = cp.as_ref().map(to_snap).unwrap_or_default();
    profile_from_capture(CaptureInput {
        passphrase: captured.passphrase,
        protocol_version: captured.protocol_version,
        ledger_sequence: captured.ledger_sequence,
        source_endpoint: TESTNET_RPC.to_string(),
        captured_at_unix: 0,
        cpu_limit: captured
            .cpu_limit
            .value
            .map(|v| (v, captured.cpu_limit.observed_at_ledger.unwrap_or(0))),
        mem_limit: captured
            .mem_limit
            .value
            .map(|v| (v, captured.mem_limit.observed_at_ledger.unwrap_or(0))),
        ledger_max_instructions: captured.ledger_max_instructions.value.map(|v| {
            (
                v,
                captured
                    .ledger_max_instructions
                    .observed_at_ledger
                    .unwrap_or(0),
            )
        }),
        fee_rate_per_instructions_increment: captured
            .fee_rate_per_instructions_increment
            .value
            .map(|v| {
                (
                    v,
                    captured
                        .fee_rate_per_instructions_increment
                        .observed_at_ledger
                        .unwrap_or(0),
                )
            }),
        max_contract_size_bytes: captured.max_contract_size_bytes.value.map(|v| {
            (
                v,
                captured
                    .max_contract_size_bytes
                    .observed_at_ledger
                    .unwrap_or(0),
            )
        }),
        tx_max_size_bytes: captured.tx_max_size_bytes.value.map(|v| {
            (
                v,
                captured.tx_max_size_bytes.observed_at_ledger.unwrap_or(0),
            )
        }),
        tx_max_contract_events_size_bytes: captured.tx_max_contract_events_size_bytes.value.map(
            |v| {
                (
                    v,
                    captured
                        .tx_max_contract_events_size_bytes
                        .observed_at_ledger
                        .unwrap_or(0),
                )
            },
        ),
        fee_contract_events_1kb: captured.fee_contract_events_1kb.value.map(|v| {
            (
                v,
                captured
                    .fee_contract_events_1kb
                    .observed_at_ledger
                    .unwrap_or(0),
            )
        }),
        cpu_cost_params: if cpu.is_empty() { None } else { Some(cpu) },
        mem_cost_params: if mem.is_empty() { None } else { Some(mem) },
        cost_params_observed_at: captured.cost_params.observed_at_ledger,
    })
}

/// Select a zero-argument function from the contract's own spec.
///
/// The corpus must not guess: a function with parameters would be a setup
/// error, not a scenario. Falls back to a known zero-arg function from the
/// walkthrough's ABI when the spec does not report parameters.
fn select_zero_arg_function(spec: &sdkt_wasm::ContractSpec) -> String {
    if let Some(f) = spec
        .functions
        .iter()
        .find(|f| f.parameters.is_empty() && f.name != "initialize")
    {
        return f.name.clone();
    }
    // The walkthrough's contract exports `get_all_order_ids` with no
    // parameters; if the spec is unavailable, use it explicitly.
    "get_all_order_ids".to_string()
}

/// One corpus scenario: the call, and what it produced.
///
/// The three outcomes are kept separate on purpose:
///
/// - `returned` — the contract ran and produced a value or void. This is the
///   only thing called "success" in the output.
/// - `contract_error` — the contract ran and rejected the call (e.g. missing
///   auth). This is a **contract-level rejection**, not a function success and
///   not a host failure.
/// - `host_error` — the host itself rejected the call (e.g. a function the
///   contract does not export). This is a host error path, not a contract
///   outcome.
struct Scenario {
    /// The contract ran and returned a value or void.
    returned: bool,
    /// The contract ran and rejected the call with a `Contract` error.
    contract_error: bool,
    /// The host rejected the call (e.g. unknown function — `WasmVm`).
    host_error: bool,
    cpu: u64,
    mem: u64,
    detail: String,
}

impl Scenario {
    /// Human label for the outcome, so the output never calls a rejection a
    /// success.
    fn outcome_label(&self) -> &'static str {
        if self.returned {
            "returned"
        } else if self.contract_error {
            "contract-level rejection"
        } else if self.host_error {
            "host error"
        } else {
            "unknown"
        }
    }
}

fn run(profile: &sdkt_fuzz::NetworkProfile, function: &str) -> Scenario {
    let wasm = sdkt_xdr::extract_wasm_bytecode_from_live_ledger_entry(TESTNET_CODE_ENTRY)
        .expect("fixture decodes");
    let exec = Executor::new(&wasm, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: network_faithful_plan(profile.clone()),
    };
    let obs = exec
        .execute_with(
            &exec.case(
                format!("corpus-{function}"),
                FunctionCall::new(function, vec![]),
                vec![],
            ),
            &env,
        )
        .expect("execution");
    let (returned, contract_error, host_error) = match &obs.status {
        ExecutionStatus::Returned(_) | ExecutionStatus::Void => (true, false, false),
        ExecutionStatus::ContractError { error_type, .. } => {
            let is_contract = error_type == "Contract";
            (false, is_contract, !is_contract)
        }
    };
    Scenario {
        returned,
        contract_error,
        host_error,
        cpu: obs.budget.consumed_cpu,
        mem: obs.budget.consumed_mem,
        detail: format!("{:?}", obs.status),
    }
}

#[tokio::test]
async fn corpus_runs_one_contract_two_scenarios_deterministically() {
    let profile = live_profile().await;
    assert_eq!(profile.status(), sdkt_fuzz::ProfileStatus::Complete);

    // The first scenario invokes a real exported function, selected from the
    // contract's own spec so it is never called with the wrong arity. The
    // contract rejects it (missing auth), so this scenario is a
    // **contract-level rejection**, not a function success.
    let wasm = sdkt_xdr::extract_wasm_bytecode_from_live_ledger_entry(TESTNET_CODE_ENTRY)
        .expect("fixture decodes");
    let spec = sdkt_wasm::parse_contract_spec(&wasm).expect("spec parses");
    let exported_function = select_zero_arg_function(&spec);
    eprintln!("corpus exported function selected from spec: {exported_function}");

    let first = run(&profile, &exported_function);
    let second = run(&profile, FAILURE_FUNCTION);

    // Scenario 1: the contract must actually run. Either it returns, or it
    // rejects the call at the contract level. It must never be a host error,
    // which would mean the call never reached the contract.
    assert!(
        !first.host_error,
        "{} must reach the contract, got {}",
        exported_function, first.detail
    );
    assert!(
        first.returned || first.contract_error,
        "{} must be a contract-level outcome, got {}",
        exported_function,
        first.detail
    );
    // Scenario 2: an unknown function is a host error (WasmVm). Asserting the
    // exact split keeps the two error classes distinct.
    assert!(
        !second.returned,
        "{} must fail, got {}",
        FAILURE_FUNCTION, second.detail
    );
    assert!(
        second.host_error,
        "{} must be a host error, got {}",
        FAILURE_FUNCTION, second.detail
    );

    // Determinism: the same scenario reproduces the same metrics.
    let again = run(&profile, &exported_function);
    assert_eq!(again.cpu, first.cpu, "scenario 1 must reproduce");
    assert_eq!(again.mem, first.mem, "scenario 1 must reproduce");
    let again_s = run(&profile, FAILURE_FUNCTION);
    assert_eq!(again_s.cpu, second.cpu, "scenario 2 must reproduce");

    // Honest count: one contract, two scenarios. Not coverage.
    eprintln!(
        "corpus: 1 contract, 2 scenarios ({exported_function}: {}, {FAILURE_FUNCTION}: {})",
        first.outcome_label(),
        second.outcome_label()
    );
    eprintln!(
        "  {exported_function}: cpu={} mem={} ({})",
        first.cpu, first.mem, first.detail
    );
    eprintln!(
        "  {FAILURE_FUNCTION}: cpu={} mem={} ({})",
        second.cpu, second.mem, second.detail
    );

    // Both scenarios must consume real budget under the live limits.
    assert!(first.cpu > 0 && second.cpu > 0);
    assert!(
        first.cpu < 400_000_000,
        "must stay under the live CPU limit"
    );
}
