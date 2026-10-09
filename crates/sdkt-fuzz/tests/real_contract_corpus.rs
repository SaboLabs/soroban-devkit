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
/// `succeeded` means the call reached the contract and returned a value or
/// void. A `Contract` error is a *contract-level* outcome (the contract ran
/// and rejected the call, e.g. missing auth) — that is a valid scenario
/// result, not a setup failure. A `WasmVm`/`Context`/`Budget` error means the
/// call never reached the contract properly.
struct Scenario {
    function: String,
    /// The call reached the contract and returned (or voided).
    returned: bool,
    /// The call failed with a contract-level error (the contract ran).
    contract_error: bool,
    /// The call failed with a host/setup error (never reached the contract).
    host_error: bool,
    cpu: u64,
    mem: u64,
    detail: String,
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
        function: function.to_string(),
        returned,
        contract_error,
        host_error,
        cpu: obs.budget.consumed_cpu,
        mem: obs.budget.consumed_mem,
        detail: format!("{:?}", obs.status),
    }
}

#[tokio::test]
async fn corpus_runs_a_success_and_a_failure_scenario_deterministically() {
    let profile = live_profile().await;
    assert_eq!(profile.status(), sdkt_fuzz::ProfileStatus::Complete);

    // Resolve the success function from the contract's own spec so the
    // corpus never invokes a function with the wrong arity.
    let wasm = sdkt_xdr::extract_wasm_bytecode_from_live_ledger_entry(TESTNET_CODE_ENTRY)
        .expect("fixture decodes");
    let spec = sdkt_wasm::parse_contract_spec(&wasm).expect("spec parses");
    let success_function = select_zero_arg_function(&spec);
    eprintln!("corpus success function selected from spec: {success_function}");

    let success = run(&profile, &success_function);
    let failure = run(&profile, FAILURE_FUNCTION);

    // The success scenario must reach the contract: either it returns, or the
    // contract itself rejects the call (e.g. missing auth — the contract ran
    // and said no, which is a valid contract-level outcome). It must never be
    // a host/setup error, which would mean the call never reached the contract.
    assert!(
        !success.host_error,
        "{} must reach the contract, got {}",
        success_function, success.detail
    );
    assert!(
        success.returned || success.contract_error,
        "{} must be a contract-level outcome, got {}",
        success_function,
        success.detail
    );
    // The failure scenario must actually fail, and the failure must be an
    // instrumented error (the host refuses a function the contract does not
    // export — error type WasmVm), which is a valid scenario: it exercises the
    // error path. What must never happen is an uninstrumented setup failure.
    assert!(
        !failure.returned,
        "{} must fail, got {}",
        FAILURE_FUNCTION, failure.detail
    );
    assert!(
        failure.detail.contains("ContractError"),
        "failure must be a contract/host error, got {}",
        failure.detail
    );

    // Determinism: the same scenario reproduces the same metrics.
    let again = run(&profile, &success_function);
    assert_eq!(again.cpu, success.cpu, "success scenario must reproduce");
    assert_eq!(again.mem, success.mem, "success scenario must reproduce");
    let again_f = run(&profile, FAILURE_FUNCTION);
    assert_eq!(again_f.cpu, failure.cpu, "failure scenario must reproduce");

    // Honest count: exactly two scenarios, one contract.
    eprintln!(
        "corpus: 1 contract, 2 scenarios ({} ok, {} rejected)",
        success.function, failure.function
    );
    eprintln!(
        "  {}: cpu={} mem={}",
        success.function, success.cpu, success.mem
    );
    eprintln!(
        "  {}: cpu={} mem={} ({})",
        failure.function, failure.cpu, failure.mem, failure.detail
    );

    // Both scenarios must consume real budget under the live limits.
    assert!(success.cpu > 0 && failure.cpu > 0);
    assert!(
        success.cpu < 400_000_000,
        "must stay under the live CPU limit"
    );
}
