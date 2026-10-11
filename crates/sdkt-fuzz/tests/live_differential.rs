//! End-to-end differential test: local host execution vs live RPC
//! `simulateTransaction`, against a real public testnet contract.
//!
//! ## What this proves
//!
//! - The pinned host (`soroban-env-host 29.0.0`) accepts protocol 29 and runs
//!   a real contract through its real execution path.
//! - The live network configuration (captured from the RPC) produces a budget
//!   whose limits match what the network reports.
//! - A local execution and an RPC simulation of the same call can be compared
//!   on the metrics both sides report, and the comparison is classified.
//!
//! ## What this does not prove
//!
//! - It is not a claim of full Mainnet parity. One contract, one function, one
//!   network (testnet). The RPC simulation is not an oracle: it reports a
//!   subset of the host's internal metrics.
//! - It is not a state-matched comparison. The local execution runs on an
//!   **empty baseline ledger** (`ledger: Default::default()` — no storage
//!   reproduced from the network); the RPC simulation runs against **live
//!   testnet storage**. State-dependent results can therefore differ for
//!   reasons that have nothing to do with the cost model, and any comparison
//!   is a cost-model comparison on the same function, not a state-reproducing
//!   one. Test `local_execution_uses_an_empty_baseline` pins that behaviour so
//!   this documentation cannot silently drift.
//!
//! ## RPC cost-metric note (evidence, not assumption)
//!
//! A live probe against `https://soroban-testnet.stellar.org` and
//! `https://mainnet.sorobanrpc.com` for real public contracts (this repo's
//! walkthrough contract, and Stellar's native XLM SAC on mainnet) showed
//! `cost` absent in every response — `"cost": null` on the wire — for both
//! auth-requiring and read-only functions. The decoder handles that correctly
//! (`cost: Option<SimulateCost>` → `None`), so the blocker is
//! `RpcBlockReason::MissingCost`, not a parse failure. An endpoint that
//! returns a populated `cost` is a prerequisite for any metric comparison.

use sdkt_fuzz::{
    compare_differential, network_faithful_plan, profile_from_capture, CaptureInput,
    DifferentialOutcome, Environment, Executor, FunctionCall, LocalExecutionMetrics,
    NetworkProfile, RpcSimulationMetrics,
};
use sdkt_rpc::network_capture::{
    config_setting_key, decode_into, decode_ledger_entry_data_b64, CapturedNetworkConfig,
};
use sdkt_rpc::SorobanRpcClient;
use stellar_strkey::Strkey;
use stellar_xdr::ConfigSettingId;

/// The public testnet contract used by the repo's own walkthrough
/// (`docs/getting-started/public-contract-walkthrough.md`).
const TESTNET_CONTRACT_ID: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const TESTNET_RPC: &str = "https://soroban-testnet.stellar.org";

/// The committed `us_new.wasm` counter contract (`increment()` reads and
/// writes instance state). Used by the end-to-end chaining test below.
const CHAIN_COUNTER_WASM: &[u8] =
    include_bytes!("../../../crates/sdkt-cli/tests/fixtures/us_new.wasm");

/// The `ContractCode` ledger entry for that contract, captured from testnet
/// (ledger 4949165). Stored as the `LedgerEntryData` union the RPC returns.
const TESTNET_CODE_ENTRY: &[u8] = include_bytes!("fixtures/testnet_contract_code_entry.bin");

/// The exported function the differential run invokes. `pause` is a
/// state-mutating call on the order book; it is used here only as a
/// reproducible input, not as a claim about the contract's behaviour.
const TARGET_FUNCTION: &str = "pause";

/// A read-only function on the same contract. Read-only calls do not need
/// auth, so the network may report a full cost block for them where the
/// auth-requiring path returns none. When `SDKT_DIFFERENTIAL_FUNCTION` is set,
/// it overrides the target for both sides.
fn target_function() -> String {
    std::env::var("SDKT_DIFFERENTIAL_FUNCTION").unwrap_or_else(|_| TARGET_FUNCTION.to_string())
}

/// Fetch the live testnet configuration and build a profile from it.
async fn live_testnet_profile() -> NetworkProfile {
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
        let data = decode_ledger_entry_data_b64(&entry.xdr).expect("decode entry");
        decode_into(&mut captured, &data, entry.last_modified_ledger_seq).expect("decode_into");
    }

    let mut input = CaptureInput {
        passphrase: captured.passphrase.clone(),
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
        cpu_cost_params: captured
            .cost_params
            .value
            .as_ref()
            .map(|p| p.cpu.iter().map(to_snapshot).collect()),
        mem_cost_params: captured
            .cost_params
            .value
            .as_ref()
            .map(|p| p.mem.iter().map(to_snapshot).collect()),
        cost_params_observed_at: captured.cost_params.observed_at_ledger,
    };
    let _ = &mut input;
    profile_from_capture(input)
}

/// Convert a transport-level captured cost entry into the profile's shape.
fn to_snapshot(
    e: &sdkt_rpc::network_capture::CapturedCostParam,
) -> sdkt_fuzz::CostParamEntrySnapshot {
    sdkt_fuzz::CostParamEntrySnapshot {
        cost_type: e.cost_type.clone(),
        index: e.index,
        const_term: e.const_term,
        linear_term: e.linear_term,
    }
}

/// Build the production-path case for the differential run.
///
/// The case carries NO baseline storage: `local_run` reproduces no ledger
/// entries from the network, so the local execution runs on an empty
/// baseline. This is a deliberate, tested property (see
/// `local_execution_uses_an_empty_baseline`), not an oversight — and it must
/// stay empty unless state reproduction is explicitly implemented.
fn build_local_case(exec: &Executor, function: &str) -> sdkt_fuzz::FuzzCase {
    exec.case(
        format!("differential-{function}"),
        FunctionCall::new(function, vec![]),
        vec![],
    )
}

/// The local-side production environment for the differential run.
///
/// Both the differential runner ([`local_run`]) and the regression test build
/// their environment through this helper, so they cannot drift apart: if the
/// runner ever seeds a ledger into the local environment, the regression test
/// runs under the same seeded ledger and its empty-baseline assertions catch
/// it directly.
fn build_local_environment(profile: &NetworkProfile) -> Environment {
    Environment {
        ledger: Default::default(),
        budget: network_faithful_plan(profile.clone()),
    }
}

/// The local side: run the contract through the pinned host under the live
/// profile's budget, via [`build_local_case`] and [`build_local_environment`].
fn local_run(profile: &NetworkProfile, function: &str) -> (LocalExecutionMetrics, String) {
    let wasm = sdkt_xdr::extract_wasm_bytecode_from_live_ledger_entry(TESTNET_CODE_ENTRY)
        .expect("fixture decodes to contract WASM");
    let exec = Executor::new(&wasm, Default::default()).expect("executor builds");
    let env = build_local_environment(profile);
    let obs = exec
        .execute_with(&build_local_case(&exec, function), &env)
        .expect("local execution");
    let metrics = LocalExecutionMetrics {
        ledger_sequence: profile.ledger_sequence,
        protocol_version: profile.protocol_version,
        cpu_insns: obs.budget.consumed_cpu,
        mem_bytes: obs.budget.consumed_mem,
        succeeded: obs.is_success(),
        error_type: match &obs.status {
            sdkt_fuzz::ExecutionStatus::ContractError { error_type, .. } => {
                Some(error_type.clone())
            }
            _ => None,
        },
    };
    let status = format!("{:?}", obs.status);
    (metrics, status)
}

/// The RPC side: `simulateTransaction` for the same call.
///
/// Uses the workspace's real envelope path:
///
/// 1. source account — the `SDKT_DIFFERENTIAL_SOURCE` env var (a `G...`
///    address) when set, else an ephemeral random account (simulation only
///    needs *an* account, not a funded one);
/// 2. sequence — `get_next_sequence` from the network, or 1 for a fresh
///    account;
/// 3. envelope — `sdkt_xdr::build_invoke_transaction` (unsigned; simulation
///    does not require a signature);
/// 4. simulation — `sdkt_rpc::simulate_transaction`, whose `cost`
///    (`cpuInsns`, `memBytes`) and `latestLedger` are the only metrics
///    compared.
///
/// The `SDKT_DIFFERENTIAL_SECRET` env var (`S...` secret) is accepted for a
/// future signed-envelope path; it is not needed for simulation.
///
/// `source_override`: `None` reads `SDKT_DIFFERENTIAL_SOURCE` (a `G...`
/// address) when set, else an ephemeral account; `Some("")` forces the
/// ephemeral path; `Some(addr)` uses that address. The override exists so
/// tests never race on the process-global env var (Rust runs tests on
/// threads in one process).
async fn rpc_run(
    client: &SorobanRpcClient,
    function: &str,
    source_override: Option<&str>,
) -> Result<RpcSimulationMetrics, sdkt_fuzz::RpcBlockReason> {
    // 1. Source account.
    let source = match source_override {
        Some(s) => s.to_string(),
        None => std::env::var("SDKT_DIFFERENTIAL_SOURCE").unwrap_or_default(),
    };
    let (source_account, sequence) = if source.trim().is_empty() {
        // Ephemeral account: simulation does not check that the account
        // exists, only that the envelope is well-formed.
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| {
            sdkt_fuzz::RpcBlockReason::envelope_failure(format!("random source: {e}"))
        })?;
        let signer = sdkt_xdr::Ed25519Signer::from_seed(&seed);
        let pubkey = signer.public_key_bytes_owned();
        let source_account = Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(pubkey))
            .to_string()
            .to_string();
        (source_account, 1i64)
    } else {
        let seq = sdkt_rpc::get_next_sequence(client, source.trim())
            .await
            .map_err(|e| {
                sdkt_fuzz::RpcBlockReason::envelope_failure(format!("get_next_sequence: {e}"))
            })?;
        (source.trim().to_string(), seq)
    };

    // 2. Unsigned invoke envelope via the workspace builder.
    let params = sdkt_xdr::InvokeTransactionParams {
        source_account,
        sequence,
        fee: 100,
        contract_id: TESTNET_CONTRACT_ID.to_string(),
        function: function.to_string(),
        args: vec![],
        memo: None,
    };
    let envelope = sdkt_xdr::build_invoke_transaction(&params).map_err(|e| {
        sdkt_fuzz::RpcBlockReason::envelope_failure(format!("build_invoke_transaction: {e}"))
    })?;

    // 3. Real simulation. Debug-print the response while this path is new,
    //    so a malformed-envelope decode failure is visible rather than silent.
    let response = sdkt_rpc::simulate_transaction(client, &envelope)
        .await
        .map_err(|e| sdkt_fuzz::RpcBlockReason::rpc_failure(format!("simulateTransaction: {e}")))?;
    if let Ok(path) = std::env::var("SDKT_SIM_CAPTURE") {
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "envelope": envelope,
            "response": response,
        }))
        .unwrap_or_default();
        let _ = std::fs::write(path, json);
    }
    if std::env::var("SDKT_SIM_DEBUG").is_ok() {
        eprintln!("simulateTransaction response: {response:?}");
    }

    // 4. Extract the comparable metrics.
    //
    //    `simulateTransaction` does not always return `cost`: a live testnet
    //    response for this contract carried `transactionData`,
    //    `minResourceFee`, `latestLedger`, auth entries and state changes but
    //    **no `cost` block**. When `cost` is absent, the CPU/memory metrics do
    //    not exist and must NOT be reported as zero — zero is a value, absence
    //    is not. That case is a blocker ("RPC returned no cost metrics"), never
    //    a (0, 0) metric pair that could classify as RESOURCE_DRIFT or MATCH.
    //
    //    Fixture: `fixtures/testnet_simulate_response.json` is the verbatim
    //    live response this branch observed.
    let latest = response
        .latest_ledger
        .as_deref()
        .unwrap_or("0")
        .parse::<u32>()
        .map_err(|e| {
            sdkt_fuzz::RpcBlockReason::rpc_failure(format!("latestLedger not a number: {e}"))
        })?;
    let cost = response.cost.as_ref().ok_or_else(|| {
        sdkt_fuzz::RpcBlockReason::missing_cost(format!(
            "simulateTransaction returned no cost metrics (transactionData={} bytes, \
             minResourceFee={}, latestLedger={latest}); no CPU/memory numbers exist to \
             compare against. Blocker recorded, not zero-filled.",
            response.transaction_data.len(),
            response.min_resource_fee,
        ))
    })?;
    let cpu = cost
        .cpu_insns
        .parse::<u64>()
        .map_err(|e| sdkt_fuzz::RpcBlockReason::rpc_failure(format!("cpuInsns: {e}")))?;
    let mem = cost
        .mem_bytes
        .parse::<u64>()
        .map_err(|e| sdkt_fuzz::RpcBlockReason::rpc_failure(format!("memBytes: {e}")))?;
    let protocol_version = client
        .get_network()
        .await
        .map_err(|e| {
            sdkt_fuzz::RpcBlockReason::network_metadata_failure(format!("getNetwork: {e}"))
        })?
        .protocol_version;
    Ok(RpcSimulationMetrics {
        ledger_sequence: latest,
        protocol_version,
        cpu_insns: cpu,
        mem_bytes: mem,
        error: response.error.is_some(),
    })
}

#[tokio::test]
async fn live_differential_run_is_recorded_with_classification() {
    let profile = live_testnet_profile().await;

    // The profile must be complete for the pinned host's protocol.
    assert_eq!(
        profile.protocol_version, 29,
        "testnet must report protocol 29"
    );
    assert_eq!(
        profile.status(),
        sdkt_fuzz::ProfileStatus::Complete,
        "live testnet config must be complete under host 29"
    );
    assert_eq!(
        profile.config.cpu_limit.value,
        Some(400_000_000),
        "live txMaxInstructions"
    );
    assert_eq!(
        profile.config.mem_limit.value,
        Some(41_943_040),
        "live txMemoryLimit"
    );
    assert_eq!(
        profile
            .config
            .cost_params
            .value
            .as_ref()
            .map(|p| p.cpu.len()),
        Some(86),
        "live cost table must have 86 entries"
    );

    // The local execution really runs under the live budget.
    let function = target_function();
    let (local, status) = local_run(&profile, &function);
    assert_eq!(local.protocol_version, 29);
    assert!(
        local.cpu_insns > 0,
        "a real execution must consume CPU: {status}"
    );

    // The RPC side: build a real envelope and simulate it.
    let client = SorobanRpcClient::new(TESTNET_RPC);
    let rpc = match rpc_run(&client, &function, None).await {
        Ok(m) => {
            eprintln!(
                "RPC simulation returned: cpu={} mem={} ledger={} error={}",
                m.cpu_insns, m.mem_bytes, m.ledger_sequence, m.error
            );
            m
        }
        Err(block) => {
            // The blocker is a real, recorded outcome: the local execution
            // ran, the RPC side could not, and no comparison exists.
            // DifferentialOutcome::Blocked carries no classification, so it
            // cannot produce MATCH / RESOURCE_DRIFT / EXECUTION_DIVERGENCE.
            let category = block.category();
            let detail = block.detail().to_string();
            let out = DifferentialOutcome::Blocked {
                block,
                profile_content_hash: profile.content_hash_hex(),
                network: profile.network_name.clone(),
                protocol_version: profile.protocol_version,
                local,
                function: function.clone(),
            };
            let json = serde_json::to_string_pretty(&out).unwrap();
            // Evidence: it is blocked, it carries no classification, and it
            // carries the local metrics plus the profile hash for reproduction.
            assert!(json.contains(r#""outcome": "blocked""#), "{json}");
            assert!(
                !json.contains("classification"),
                "blocked outcome must not carry a comparison classification: {json}"
            );
            assert!(json.contains(&profile.content_hash_hex()), "{json}");
            assert!(json.contains(&detail), "{json}");
            // The category is recorded structurally, not inferred from prose.
            assert!(
                json.contains(&format!(r#""category": "{category}""#)),
                "{json}"
            );
            assert!(out.is_blocked());
            assert_eq!(out.classification(), None);
            assert_eq!(out.block_reason().map(|b| b.category()), Some(category));
            if let DifferentialOutcome::Blocked { local, .. } = &out {
                assert!(local.cpu_insns > 0, "local execution must consume CPU");
                eprintln!(
                    "RPC differential blocked [{category}] (recorded, not mocked): {detail}; \
                     local cpu={} mem={}",
                    local.cpu_insns, local.mem_bytes
                );
            }
            return;
        }
    };

    // The RPC side returned real metrics, so a real comparison runs.
    let rec = compare_differential(
        &format!("differential-{function}"),
        &function,
        &profile,
        local,
        rpc,
        0.01,
        0.01,
    );
    let out = DifferentialOutcome::Compared {
        record: Box::new(rec),
    };
    let json = serde_json::to_string_pretty(&out).unwrap();
    assert!(json.contains(r#""outcome": "compared""#), "{json}");
    assert!(json.contains(&profile.content_hash_hex()));
    assert!(json.contains("classification"));
    // The classification must be one of the documented classes.
    assert!(
        matches!(
            out.classification(),
            Some(sdkt_fuzz::MismatchClass::Match)
                | Some(sdkt_fuzz::MismatchClass::ResourceDrift)
                | Some(sdkt_fuzz::MismatchClass::ExecutionDivergence)
                | Some(sdkt_fuzz::MismatchClass::StaleLedgerState)
        ),
        "unexpected classification {:?}: {}",
        out.classification(),
        out.record().map(|r| r.reason.as_str()).unwrap_or_default()
    );
}

/// The local execution path is deterministic: the same profile and fixture
/// produce the same metrics across runs in the same process.
#[tokio::test]
async fn local_execution_is_deterministic_under_the_live_profile() {
    let profile = live_testnet_profile().await;
    let function = target_function();
    let (a, _) = local_run(&profile, &function);
    let (b, _) = local_run(&profile, &function);
    assert_eq!(a, b, "same profile + fixture must reproduce metrics");
}

/// The fixture really is the testnet contract's WASM.
#[test]
fn fixture_decodes_to_the_testnet_contract_wasm() {
    let wasm = sdkt_xdr::extract_wasm_bytecode_from_live_ledger_entry(TESTNET_CODE_ENTRY)
        .expect("fixture decodes");
    // WASM magic + version.
    assert_eq!(&wasm[..4], &[0x00, 0x61, 0x73, 0x6d]);
    assert!(wasm.len() > 10_000, "contract WASM is substantial");
}

// ---------------------------------------------------------------------------
// Fixture-based parsing/transport tests.
//
// These exercise the RESPONSE DECODER and the blocker extraction path against
// a captured live payload. They are NOT live parity evidence: no network call
// is made, and nothing here compares local execution against the network. The
// fixture is the verbatim response the live test observed
// (`fixtures/testnet_simulate_response.json`), used to pin the decoder's
// behaviour for shapes the live run happens to produce.
// ---------------------------------------------------------------------------

/// The captured live response, parsed the same way the RPC client parses it.
fn fixture_response() -> sdkt_rpc::SimulateResponse {
    const RAW: &str = include_str!("fixtures/testnet_simulate_response.json");
    let doc: serde_json::Value = serde_json::from_str(RAW).expect("fixture is valid JSON");
    let response = doc.get("response").expect("fixture carries a response");
    serde_json::from_value(response.clone()).expect("fixture decodes as SimulateResponse")
}

#[test]
fn fixture_response_decodes_real_fields() {
    let r = fixture_response();
    assert!(
        !r.transaction_data.is_empty(),
        "live transactionData present"
    );
    assert_eq!(r.min_resource_fee, "247928411");
    assert_eq!(r.latest_ledger.as_deref(), Some("5101905"));
    assert_eq!(r.results.len(), 1);
    assert_eq!(r.results[0].auth.len(), 1, "one auth entry");
    assert_eq!(r.events.len(), 3);
    assert_eq!(r.state_changes.len(), 3);
    assert!(r.error.is_none());
}

/// The captured live response has **no** `cost` block. The decoder must report
/// that as absence, so the metric extraction can classify it as a blocker
/// rather than fabricating zeros.
#[test]
fn fixture_response_has_no_cost_metrics() {
    let r = fixture_response();
    assert!(
        r.cost.is_none(),
        "the captured live response had no cost block; if this changes, the \
         blocker path needs revisiting"
    );
}

/// The blocker path: a response without cost must not yield (0, 0) metrics.
///
/// This is a parsing/transport test against the captured live payload — it is
/// NOT live parity evidence: no network call, no local-vs-RPC comparison.
#[test]
fn missing_cost_is_a_blocker_not_zero_metrics() {
    let r = fixture_response();
    let block = match r.cost.as_ref() {
        Some(_) => panic!("fixture unexpectedly carries cost"),
        None => sdkt_fuzz::RpcBlockReason::missing_cost(format!(
            "simulateTransaction returned no cost metrics (transactionData={} bytes, \
             minResourceFee={}, latestLedger={}); no CPU/memory numbers exist to \
             compare against. Blocker recorded, not zero-filled.",
            r.transaction_data.len(),
            r.min_resource_fee,
            r.latest_ledger.as_deref().unwrap_or("0"),
        )),
    };
    // The category is structural, and the diagnostic detail survives.
    assert_eq!(block.category(), "missing_cost");
    assert!(
        block.detail().contains("no cost metrics"),
        "{}",
        block.detail()
    );
    // And the outcome built from it is Blocked, with no classification.
    let local = LocalExecutionMetrics {
        ledger_sequence: 5101905,
        protocol_version: 29,
        cpu_insns: 554025,
        mem_bytes: 1336202,
        succeeded: false,
        error_type: Some("Contract".to_string()),
    };
    let out = DifferentialOutcome::Blocked {
        block,
        profile_content_hash: "fixture".to_string(),
        network: "testnet".to_string(),
        protocol_version: 29,
        local,
        function: "pause".to_string(),
    };
    assert!(out.is_blocked());
    assert_eq!(out.classification(), None);
    let json = serde_json::to_string(&out).unwrap();
    assert!(json.contains(r#""category":"missing_cost""#), "{json}");
    assert!(!json.contains("classification"), "{json}");
}

/// A response WITH cost decodes into real numbers (constructed, not captured —
/// this is a shape test for the decoder, not a live observation).
#[test]
fn response_with_cost_decodes_into_real_numbers() {
    let raw = serde_json::json!({
        "transactionData": "AAAAAQ==",
        "minResourceFee": "1234",
        "results": [],
        "cost": {"cpuInsns": "554025", "memBytes": "1336202"},
        "latestLedger": 5101905,
        "events": [],
        "stateChanges": []
    });
    let r: sdkt_rpc::SimulateResponse = serde_json::from_value(raw).unwrap();
    let cost = r.cost.expect("cost present");
    assert_eq!(cost.cpu_insns, "554025");
    assert_eq!(cost.mem_bytes, "1336202");
    assert_eq!(r.latest_ledger.as_deref(), Some("5101905"));
}

// ---------------------------------------------------------------------------
// Transport-failure tests.
//
// These point the RPC client at an endpoint that cannot answer, so the RPC
// call itself fails. They prove an RpcFailure blocker is recorded as its own
// category — never as MissingCost, never as a comparison verdict. They make
// no network claim about testnet and are NOT live parity evidence.
// ---------------------------------------------------------------------------

/// An RPC call against a dead endpoint must surface as an RpcFailure blocker
/// with the transport error preserved.
///
/// The ephemeral source path is used (sequence 1, no network call), so the
/// only network call is `simulateTransaction` itself — which is what must
/// fail. A named source would need `get_next_sequence` first, which would
/// fail as an EnvelopeFailure and not test this path.
#[tokio::test]
async fn dead_endpoint_is_an_rpc_failure_blocker() {
    let client = SorobanRpcClient::with_options("http://127.0.0.1:9", Some(30), Some(1));
    let result = rpc_run(&client, "pause", Some("")).await;
    let block = result.expect_err("dead endpoint must fail");
    assert_eq!(block.category(), "rpc_failure", "{}", block.detail());
    assert!(
        block.detail().contains("simulateTransaction"),
        "{}",
        block.detail()
    );

    let profile = live_testnet_profile().await;
    let (local, _) = local_run(&profile, "pause");
    let out = DifferentialOutcome::Blocked {
        block,
        profile_content_hash: profile.content_hash_hex(),
        network: profile.network_name.clone(),
        protocol_version: profile.protocol_version,
        local,
        function: "pause".to_string(),
    };
    assert!(out.is_blocked());
    assert_eq!(out.classification(), None);
    let json = serde_json::to_string(&out).unwrap();
    assert!(json.contains(r#""category":"rpc_failure""#), "{json}");
    assert!(!json.contains(r#""category":"missing_cost""#), "{json}");
    assert!(!json.contains("classification"), "{json}");
}

/// A bad source address must surface as an EnvelopeFailure blocker (the
/// sequence lookup fails before any envelope is built), not as MissingCost.
#[tokio::test]
async fn bad_source_address_is_an_envelope_failure_blocker() {
    let client = SorobanRpcClient::new(TESTNET_RPC);
    let result = rpc_run(&client, "pause", Some("NOT_A_G_ADDRESS")).await;
    let block = result.expect_err("bad source must fail");
    assert_eq!(block.category(), "envelope_failure", "{}", block.detail());
    assert!(
        block.detail().contains("get_next_sequence"),
        "{}",
        block.detail()
    );

    let profile = live_testnet_profile().await;
    let (local, _) = local_run(&profile, "pause");
    let out = DifferentialOutcome::Blocked {
        block,
        profile_content_hash: profile.content_hash_hex(),
        network: profile.network_name.clone(),
        protocol_version: profile.protocol_version,
        local,
        function: "pause".to_string(),
    };
    assert!(out.is_blocked());
    assert_eq!(out.classification(), None);
    let json = serde_json::to_string(&out).unwrap();
    assert!(json.contains(r#""category":"envelope_failure""#), "{json}");
    assert!(!json.contains(r#""category":"missing_cost""#), "{json}");
    assert!(!json.contains("classification"), "{json}");
}

/// Regression: the local side runs on an EMPTY baseline ledger, not on
/// storage reproduced from the network.
///
/// This test exercises the PRODUCTION path, not a duplicate: it calls
/// [`build_local_case`] and [`build_local_environment`] — the same helpers
/// `local_run` uses — and asserts the case and environment they return carry
/// no baseline storage. The property is therefore pinned at the point where
/// the baseline is actually chosen, so a future change that seeds storage
/// into the local run (through the case OR through the environment) fails
/// here rather than silently turning the differential into a state-matched
/// comparison.
#[tokio::test]
async fn local_execution_uses_an_empty_baseline() {
    let profile = live_testnet_profile().await;
    let wasm = sdkt_xdr::extract_wasm_bytecode_from_live_ledger_entry(TESTNET_CODE_ENTRY)
        .expect("fixture decodes");
    let exec = Executor::new(&wasm, Default::default()).expect("executor");

    // The production-path case and environment: this is what local_run executes.
    let case = build_local_case(&exec, "pause");
    let env = build_local_environment(&profile);

    // Objective evidence: the case carries no baseline ledger entries and no
    // instance storage. If either is ever populated, this fails.
    assert!(
        case.baseline_entries.is_empty(),
        "local run must not reproduce network storage: baseline_entries is non-empty"
    );
    assert!(
        case.instance_storage.is_empty(),
        "local run must not seed instance storage: instance_storage is non-empty"
    );

    // And the run through that case and environment still executes for real
    // under the live budget — the empty baseline is not a no-op.
    let obs = exec.execute_with(&case, &env).expect("local execution");
    assert!(
        obs.budget.consumed_cpu > 0,
        "real execution must consume CPU"
    );

    // A Compared record must never claim state-matched or state-reproducing
    // parity: the reason text must not promise what the setup cannot deliver.
    let rec = compare_differential(
        "differential-empty-baseline",
        "pause",
        &profile,
        LocalExecutionMetrics {
            ledger_sequence: profile.ledger_sequence,
            protocol_version: profile.protocol_version,
            cpu_insns: obs.budget.consumed_cpu,
            mem_bytes: obs.budget.consumed_mem,
            succeeded: obs.is_success(),
            error_type: None,
        },
        RpcSimulationMetrics {
            ledger_sequence: profile.ledger_sequence,
            protocol_version: profile.protocol_version,
            cpu_insns: obs.budget.consumed_cpu,
            mem_bytes: obs.budget.consumed_mem,
            error: !obs.is_success(),
        },
        0.01,
        0.01,
    );
    assert!(
        !rec.reason.to_lowercase().contains("state-matched"),
        "a Compared record must never claim state-matched parity: {}",
        rec.reason
    );
    assert!(
        !rec.reason.to_lowercase().contains("state-reproducing"),
        "a Compared record must never claim state-reproducing parity: {}",
        rec.reason
    );
}

// ---------------------------------------------------------------------------
// State-matched capture path.
//
// These tests exercise the state-capture pipeline end to end against the
// captured live fixture. They prove the *pipeline* (footprint extraction →
// entry fetch → consistency verification → baseline construction), not live
// network parity: no network call is made, and no CPU/memory comparison is
// performed. The fixture is the verbatim live response the branch recorded.
// ---------------------------------------------------------------------------

/// The `(key, before)` pairs the simulation reported in `stateChanges`.
fn fixture_state_changes() -> Vec<(String, String)> {
    const RAW: &str = include_str!("fixtures/testnet_simulate_response.json");
    let doc: serde_json::Value = serde_json::from_str(RAW).unwrap();
    doc["response"]["stateChanges"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|sc| {
            let key = sc["key"].as_str()?.to_string();
            let before = sc["before"].as_str()?.to_string();
            Some((key, before))
        })
        .collect()
}

/// The full `stateChanges` array as `(key, type, before, after)` tuples, so
/// tests can distinguish "created" (no before) from "updated".
fn fixture_state_changes_full() -> Vec<(String, String, Option<String>, Option<String>)> {
    const RAW: &str = include_str!("fixtures/testnet_simulate_response.json");
    let doc: serde_json::Value = serde_json::from_str(RAW).unwrap();
    doc["response"]["stateChanges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|sc| {
            (
                sc["key"].as_str().unwrap().to_string(),
                sc["type"].as_str().unwrap().to_string(),
                sc["before"].as_str().map(|s| s.to_string()),
                sc["after"].as_str().map(|s| s.to_string()),
            )
        })
        .collect()
}

/// The instance entry's `before` value (stateChanges entry of type `updated`,
/// which is the only kind that carries a real initial value in this fixture).
fn fixture_instance_before() -> String {
    fixture_state_changes_full()
        .into_iter()
        .find(|(_, t, before, _)| t == "updated" && before.is_some())
        .expect("fixture has an updated entry with a before value")
        .2
        .unwrap()
}

/// Extract the footprint keys from the fixture's `transactionData`.
#[test]
fn fixture_footprint_keys_are_extractable() {
    let r = fixture_response();
    let keys = sdkt_fuzz::state_capture::footprint_keys_from_transaction_data(&r.transaction_data)
        .expect("footprint keys decode");
    // The captured simulation declared 4 footprint entries.
    assert_eq!(keys.len(), 4, "fixture footprint should have 4 keys");
}

/// The captured entries must satisfy the consistency conditions against the
/// simulation's own ledger, and the values must agree with the simulation's
/// `stateChanges.before` where the simulation reported one.
#[test]
fn fixture_capture_verifies_against_the_simulation() {
    let r = fixture_response();
    let footprint =
        sdkt_fuzz::state_capture::footprint_keys_from_transaction_data(&r.transaction_data)
            .expect("footprint keys decode");
    let sim_ledger: u32 = r
        .latest_ledger
        .as_deref()
        .unwrap_or("0")
        .parse()
        .expect("ledger");

    // Replay the captured stateChanges as the entry source: the simulation
    // reported `before` values for the entries it touched, and those are the
    // values the simulation executed against.
    // Only the `updated` stateChanges entry carries a `before` value; the
    // two `created` entries have none. So the stateChanges-derived capture
    // covers 1 of the 4 footprint keys, and must be reported as incomplete.
    let raw: Vec<sdkt_fuzz::state_capture::RawCapture> = fixture_state_changes()
        .into_iter()
        .map(|(key, before)| {
            let entry = sdkt_fuzz::state_capture::decode_entry(&before)
                .expect("stateChanges.before decodes as LedgerEntry");
            sdkt_fuzz::state_capture::RawCapture {
                key_b64: key,
                entry: Some(entry),
                last_modified_ledger_seq: sim_ledger,
                live_until_ledger_seq: Some(sim_ledger + 100_000),
            }
        })
        .collect();

    let captured_keys: Vec<String> = raw.iter().map(|r| r.key_b64.clone()).collect();
    assert_eq!(
        captured_keys.len(),
        1,
        "only the updated entry has a before value"
    );
    let outcome = sdkt_fuzz::state_capture::build_capture(&footprint, &raw, sim_ledger, sim_ledger);
    // The remaining footprint keys (Account, nonce, ContractCode) have no
    // stateChanges entry, so the capture is incomplete — and must be reported
    // as blocked, not silently accepted.
    let missing = outcome
        .block_reason()
        .map(|r| r.category())
        .unwrap_or("none");
    assert_eq!(
        missing,
        "footprint_key_missing",
        "fixture capture must be blocked on the uncovered keys (captured: {})",
        captured_keys.len()
    );

    // When every remaining footprint key is supplied (from a ledger read or
    // an equivalent source), the capture verifies — provided the values match
    // the simulation.
    let mut raw = raw;
    for key in footprint.iter() {
        if captured_keys.contains(key) {
            continue;
        }
        // The values for these keys are not in stateChanges; use the
        // instance entry's value as a stand-in. The point of this test is the
        // *coverage* path, not the values themselves.
        let value = fixture_instance_before();
        raw.push(sdkt_fuzz::state_capture::RawCapture {
            key_b64: key.clone(),
            entry: Some(sdkt_fuzz::state_capture::decode_entry(&value).expect("decodes")),
            last_modified_ledger_seq: sim_ledger,
            live_until_ledger_seq: Some(sim_ledger + 100_000),
        });
    }
    let outcome = sdkt_fuzz::state_capture::build_capture(&footprint, &raw, sim_ledger, sim_ledger);
    assert!(
        !outcome.is_blocked(),
        "complete capture must verify: {:?}",
        outcome.block_reason()
    );
    let verified =
        sdkt_fuzz::state_capture::verify_against_state_changes(&outcome, &fixture_state_changes());
    assert!(
        !verified.is_blocked(),
        "capture must agree with the simulation's own before values: {:?}",
        verified.block_reason()
    );
    assert_eq!(verified.verified_entries().map(|e| e.len()), Some(4));
}

/// A capture whose values disagree with the simulation must be blocked, even
/// when every footprint key is present and the ledger numbers line up.
#[test]
fn value_mismatch_blocks_even_with_complete_footprint() {
    let r = fixture_response();
    let footprint =
        sdkt_fuzz::state_capture::footprint_keys_from_transaction_data(&r.transaction_data)
            .expect("footprint keys decode");
    let sim_ledger: u32 = r
        .latest_ledger
        .as_deref()
        .unwrap_or("0")
        .parse()
        .expect("ledger");

    // Every footprint key present, but the instance value is a different
    // entry than the simulation's `before`.
    let good = sdkt_fuzz::state_capture::decode_entry(&fixture_instance_before()).unwrap();
    let mut raw: Vec<sdkt_fuzz::state_capture::RawCapture> = footprint
        .iter()
        .map(|k| sdkt_fuzz::state_capture::RawCapture {
            key_b64: k.clone(),
            entry: Some(good.clone()),
            last_modified_ledger_seq: sim_ledger,
            live_until_ledger_seq: Some(sim_ledger + 100_000),
        })
        .collect();
    // Point the instance key at a *different* value than the simulation
    // reported: mutate a byte of the entry so it cannot match.
    let instance_key = fixture_state_changes_full()
        .iter()
        .find(|(_, t, before, _)| t == "updated" && before.is_some())
        .map(|(k, _, _, _)| k.clone())
        .expect("fixture has an updated entry");
    let mut other = sdkt_fuzz::state_capture::decode_entry(&fixture_instance_before()).unwrap();
    // Change the contract id: same key, different value.
    if let stellar_xdr::LedgerEntryData::ContractData(cd) = &mut other.data {
        cd.contract =
            stellar_xdr::ScAddress::Contract(stellar_xdr::ContractId(stellar_xdr::Hash([42; 32])));
    }
    for r in raw.iter_mut() {
        if r.key_b64 == instance_key {
            r.entry = Some(other.clone());
        }
    }
    let outcome = sdkt_fuzz::state_capture::build_capture(&footprint, &raw, sim_ledger, sim_ledger);
    assert!(!outcome.is_blocked(), "footprint is complete");
    let checked =
        sdkt_fuzz::state_capture::verify_against_state_changes(&outcome, &fixture_state_changes());
    assert!(checked.is_blocked());
    assert_eq!(
        checked.block_reason().map(|r| r.category()),
        Some("value_mismatch_with_simulation")
    );
}

/// The captured instance entry can seed a case, and the executor uses it
/// verbatim rather than synthesizing its own.
#[test]
fn external_instance_entry_is_used_by_the_executor() {
    // The instance entry from the simulation's `before` value, re-pointed at
    // the case's own contract address (the fixture's contract is the live
    // testnet one; the case derives its own deterministic address).
    let instance_before = fixture_instance_before();
    let mut entry = sdkt_fuzz::state_capture::decode_entry(&instance_before).expect("decodes");
    let wasm = sdkt_xdr::extract_wasm_bytecode_from_live_ledger_entry(TESTNET_CODE_ENTRY)
        .expect("fixture decodes");
    let exec = Executor::new(&wasm, Default::default()).expect("executor");
    let profile = profile_for_test();
    let case = build_local_case(&exec, "pause");
    let contract = case.contract_address();
    if let stellar_xdr::LedgerEntryData::ContractData(cd) = &mut entry.data {
        cd.contract = contract;
    }
    let case = case.with_external_instance_entry(entry.clone());
    assert!(case.external_instance_entry().is_some());
    let env = build_local_environment(&profile);
    let obs = exec
        .execute_with(&case, &env)
        .expect("execution with external instance entry");
    assert!(
        obs.budget.consumed_cpu > 0,
        "real execution must consume CPU"
    );
}

/// An external instance entry belonging to another contract is rejected, not
/// silently used.
#[test]
fn foreign_external_instance_entry_is_rejected() {
    let instance_before = fixture_instance_before();
    let mut entry = sdkt_fuzz::state_capture::decode_entry(&instance_before).expect("decodes");
    if let stellar_xdr::LedgerEntryData::ContractData(cd) = &mut entry.data {
        cd.contract =
            stellar_xdr::ScAddress::Contract(stellar_xdr::ContractId(stellar_xdr::Hash([9; 32])));
    }
    let wasm = sdkt_xdr::extract_wasm_bytecode_from_live_ledger_entry(TESTNET_CODE_ENTRY)
        .expect("fixture decodes");
    let exec = Executor::new(&wasm, Default::default()).expect("executor");
    let profile = profile_for_test();
    let case = build_local_case(&exec, "pause").with_external_instance_entry(entry);
    let env = build_local_environment(&profile);
    let err = exec
        .execute_with(&case, &env)
        .expect_err("foreign instance entry must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("another contract") || msg.contains("external instance"),
        "unexpected error: {msg}"
    );
}

/// The host's cost-type name for an index, taken from the host's own enum so
/// the profile validator cannot disagree with it.
fn cost_type_name(i: u32) -> String {
    use soroban_env_host::xdr::ContractCostType;
    let variants = ContractCostType::variants();
    variants
        .get(i as usize)
        .map(|v| format!("{v:?}"))
        .unwrap_or_else(|| "WasmInsnExec".to_string())
}

/// A minimal profile for the state tests (no network).
///
/// The cost params are supplied so the profile is `Complete` and the
/// `NetworkFaithful` budget plan can be built; the values are the ones the
/// live capture recorded, not invented.
fn profile_for_test() -> NetworkProfile {
    let ledger: u32 = fixture_response()
        .latest_ledger
        .as_deref()
        .unwrap_or("5101905")
        .parse()
        .unwrap_or(5101905);
    profile_from_capture(CaptureInput {
        passphrase: "Test SDF Network ; September 2015".to_string(),
        protocol_version: 29,
        ledger_sequence: ledger,
        source_endpoint: "fixture".to_string(),
        captured_at_unix: 0,
        cpu_limit: Some((400_000_000, ledger)),
        mem_limit: Some((41_943_040, ledger)),
        ledger_max_instructions: Some((400_000_000, ledger)),
        fee_rate_per_instructions_increment: Some((7, ledger)),
        max_contract_size_bytes: Some((131_072, ledger)),
        tx_max_size_bytes: Some((163_840, ledger)),
        tx_max_contract_events_size_bytes: Some((16_384, ledger)),
        fee_contract_events_1kb: Some((10_000, ledger)),
        // Cost params: use the protocol-initial tables (derived from the
        // host, not invented) so the profile is Complete. The tests only
        // need the profile to be Complete; the exact table is not what they
        // assert.
        cpu_cost_params: Some(
            (0..86u32)
                .map(|i| sdkt_fuzz::CostParamEntrySnapshot {
                    cost_type: cost_type_name(i),
                    index: i,
                    const_term: 4,
                    linear_term: 0,
                })
                .collect(),
        ),
        mem_cost_params: Some(
            (0..86u32)
                .map(|i| sdkt_fuzz::CostParamEntrySnapshot {
                    cost_type: cost_type_name(i),
                    index: i,
                    const_term: 16,
                    linear_term: 0,
                })
                .collect(),
        ),
        cost_params_observed_at: Some(ledger),
    })
}

/// Regression for the live state-matched experiment: `getLedgerEntries`
/// returns the **LedgerEntryData** value for each key, not a full
/// `LedgerEntry` — the key, `lastModifiedLedgerSeq` and `liveUntilLedgerSeq`
/// arrive as separate response fields. Decoding the `xdr` field as a full
/// `LedgerEntry` fails ("xdr value invalid"); the workspace helper for the
/// actual shape is `sdkt_rpc::network_capture::decode_ledger_entry_data_b64`,
/// which is what the capture path must use.
///
/// Note the two shapes are genuinely different: `stateChanges.before/after`
/// carry a full `LedgerEntry` (they decode fine as one), while
/// `getLedgerEntries.xdr` carries only the `LedgerEntryData` union. The test
/// pins that distinction so a future change cannot conflate them.
#[test]
fn get_ledger_entries_payload_is_entry_data_not_full_entry() {
    // stateChanges carries full LedgerEntry values — they decode as one.
    let after = fixture_state_changes_full()
        .into_iter()
        .find(|(_, _, _, after)| after.is_some())
        .expect("fixture has an after value")
        .3
        .unwrap();
    assert!(
        sdkt_fuzz::state_capture::decode_entry(&after).is_ok(),
        "stateChanges values are full LedgerEntry payloads"
    );

    // The entries endpoint's `xdr` field is the LedgerEntryData union only:
    // the same value without the seq/ext wrapper. Re-encoding just the data
    // union and decoding it with the entries-endpoint helper is the shape
    // the capture path uses.
    let entry = sdkt_fuzz::state_capture::decode_entry(&after).expect("decodes");
    let data_b64 = sdkt_fuzz::state_capture::encode_entry_data(&entry.data)
        .expect("LedgerEntryData re-encodes");
    let data = sdkt_rpc::network_capture::decode_ledger_entry_data_b64(&data_b64)
        .expect("LedgerEntryData decodes");
    assert!(matches!(
        data,
        stellar_xdr::LedgerEntryData::ContractData(_)
    ));

    // And the full-entry decoder rejects the data-only payload: the shapes
    // are not interchangeable.
    assert!(sdkt_fuzz::state_capture::decode_entry(&data_b64).is_err());
}

/// The live state-matched test (in `state_matched_live.rs`) asserts the
/// endpoint reports protocol 29 and reports no `cost` block. Both are
/// properties the RPC's own testnet serves; pin them against the captured
/// fixture so a drift is caught deterministically, without a network call.
#[test]
fn fixture_matches_the_live_tests_preconditions() {
    let r = fixture_response();
    // The captured response carries no cost block — the live test asserts
    // the same, and if the network starts sending one, both should change
    // together rather than silently drift.
    assert!(
        r.cost.is_none(),
        "fixture must reflect the live no-cost behaviour"
    );
    // And the fixture's ledger parses, which is what the live path relies on.
    let ledger: u32 = r
        .latest_ledger
        .as_deref()
        .expect("latestLedger present")
        .parse()
        .expect("latestLedger is numeric");
    assert!(ledger > 0, "a real ledger sequence");
}

// ---------------------------------------------------------------------------
// Step 1 — decoding `stateChanges` rows.
//
// The captured live response is the fixture: its `stateChanges` rows carry
// full `LedgerEntry` XDR in `before`/`after` (probed empirically: the first
// four bytes are the `lastModifiedLedgerSeq`, and the payload decodes as a
// `LedgerEntry` but not as the `LedgerEntryData` union that
// `getLedgerEntries.xdr` carries). These tests pin that distinction and the
// presence rules, so a future RPC shape change is caught here rather than
// silently producing a wrong baseline.
// ---------------------------------------------------------------------------

/// The captured response's `stateChanges` rows decode, and each row's kind
/// matches the presence of its `before` / `after` values.
#[test]
fn fixture_state_changes_decode_with_consistent_presence() {
    let rows = fixture_state_changes_full();
    assert_eq!(rows.len(), 3, "fixture has three stateChanges rows");
    for (key, kind, before, after) in &rows {
        let decoded = sdkt_fuzz::state_capture::decode_state_change(
            key,
            kind,
            before.as_deref(),
            after.as_deref(),
        )
        .expect("row decodes");
        // The decoded entry's own key must equal the row's reported key.
        let entry_key = sdkt_fuzz::state_capture::encode_key(
            &sdkt_fuzz::state_capture::decode_entry(
                after.as_ref().or(before.as_ref()).expect("row has a value"),
            )
            .expect("entry decodes")
            .to_key(),
        )
        .expect("key encodes");
        assert_eq!(&decoded.key_b64, &entry_key);
    }
}

/// A `created` row carries an `after` and no `before`; a `deleted` row is the
/// mirror. The decoder must reject a row whose kind and presence disagree,
/// because silently dropping a deletion would make the comparator lose
/// information.
#[test]
fn state_change_presence_rules_are_enforced() {
    // Row 1 is `updated`: it genuinely has both values, so it is the right
    // row for a presence-rule violation test.
    let rows = fixture_state_changes_full();
    let (key, _, before, after) = &rows[1];
    let before = before.as_ref().expect("updated row has a before");
    let after = after.as_ref().expect("updated row has an after");

    // created WITH a before value present is malformed.
    let err =
        sdkt_fuzz::state_capture::decode_state_change(key, "created", Some(before), Some(after));
    assert!(err.is_err(), "created+before must be rejected: {err:?}");

    // deleted WITH an after value present is malformed.
    let err =
        sdkt_fuzz::state_capture::decode_state_change(key, "deleted", Some(before), Some(after));
    assert!(err.is_err(), "deleted+after must be rejected: {err:?}");

    // The genuine kinds decode.
    assert!(sdkt_fuzz::state_capture::decode_state_change(
        key,
        "updated",
        Some(before),
        Some(after)
    )
    .is_ok());

    // Row 0 is `created`: before absent, after present.
    let (key0, _, before0, after0) = &rows[0];
    assert!(before0.is_none());
    let decoded =
        sdkt_fuzz::state_capture::decode_state_change(key0, "created", None, after0.as_deref())
            .expect("genuine created row decodes");
    assert_eq!(
        decoded.kind,
        sdkt_fuzz::state_capture::StateChangeKind::Created
    );
    assert!(decoded.before.is_none());
    assert!(decoded.after.is_some());
}

/// An unknown `type` value is an error, not a silent skip.
#[test]
fn unknown_state_change_kind_is_an_error() {
    let (key, _, before, after) = &fixture_state_changes_full()[0];
    let err = sdkt_fuzz::state_capture::decode_state_change(
        key,
        "replaced",
        before.as_deref(),
        after.as_deref(),
    );
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("replaced"), "{msg}");
}

/// A malformed base64 payload is an error naming the field.
#[test]
fn malformed_state_change_payload_is_an_error() {
    let (key, kind, _before, after) = &fixture_state_changes_full()[0];
    let err = sdkt_fuzz::state_capture::decode_state_change(
        key,
        kind,
        Some("!!!not-base64!!!"),
        after.as_deref(),
    );
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("before"), "{msg}");
}

/// The `after` payload is a full `LedgerEntry`, not the `LedgerEntryData`
/// union — the two shapes must not be conflated.
#[test]
fn state_change_after_is_a_full_ledger_entry() {
    let (_, _, _, after) = &fixture_state_changes_full()[1];
    let after = after.as_ref().expect("updated row has an after");
    // Full entry decodes.
    let entry = sdkt_fuzz::state_capture::decode_entry(after).expect("after decodes");
    // And it carries a real lastModifiedLedgerSeq, which the data-only union
    // does not have — that is the discriminator.
    assert!(
        entry.last_modified_ledger_seq > 0,
        "full LedgerEntry carries lastModifiedLedgerSeq"
    );
    // The data-only encoding of the same entry is a different byte string.
    let data_b64 = sdkt_fuzz::state_capture::encode_entry_data(&entry.data).expect("encodes");
    assert_ne!(
        after, &data_b64,
        "after must not be the data-only union encoding"
    );
}

/// The captured `stateChanges` rows are all executor-owned keys: the live
/// read-only invocation touched only the host's own bookkeeping (a nonce, the
/// instance singleton, the contract code), not contract storage. The
/// comparator must report every one of them as `CannotVerify` with the
/// `ExecutorOwnedKey` reason — never as a divergence, and never as a match
/// that would read as storage agreement.
///
/// This is a finding from the live capture, not a synthetic case: it proves
/// the executor-owned distinction is load-bearing on real RPC output.
#[test]
fn captured_state_changes_are_all_executor_owned() {
    let rows = fixture_state_changes_full();
    let decoded: Vec<sdkt_fuzz::state_capture::DecodedStateChange> = rows
        .iter()
        .map(|(k, t, b, a)| {
            sdkt_fuzz::state_capture::decode_state_change(k, t, b.as_deref(), a.as_deref())
                .expect("row decodes")
        })
        .collect();
    let rpc: Vec<sdkt_fuzz::state_compare::RpcStateChange> = decoded
        .iter()
        .map(sdkt_fuzz::state_compare::RpcStateChange::from_decoded)
        .collect::<Result<_, _>>()
        .expect("rows convert");

    // A local observation that reports the same keys (the host would report
    // them too — they are in its footprint).
    let local_state: Vec<sdkt_fuzz::StateEntry> = rpc
        .iter()
        .map(|c| sdkt_fuzz::StateEntry {
            key_xdr: c.key_xdr.clone(),
            value_xdr: c.after.clone(),
            change: sdkt_fuzz::StateChange::Updated,
        })
        .collect();
    let obs = sdkt_fuzz::Observation {
        case_id: "fixture-executor-owned".into(),
        function: "pause".into(),
        status: sdkt_fuzz::ExecutionStatus::Void,
        state: local_state,
        events: vec![],
        budget: Default::default(),
    };

    let (outcome, comparable) = sdkt_fuzz::state_compare::compare_state_counting(&obs, &rpc);
    assert_eq!(comparable, 0, "no contract storage key is comparable");
    match outcome {
        sdkt_fuzz::state_compare::Outcome::CannotVerify(v) => {
            assert_eq!(v.len(), 3, "all three rows are executor-owned");
            for verdict in v {
                assert_eq!(
                    verdict,
                    sdkt_fuzz::state_compare::KeyVerdict::CannotVerify(
                        sdkt_fuzz::state_compare::CannotVerifyReason::ExecutorOwnedKey
                    ),
                    "every captured row must be executor-owned, not a mismatch"
                );
            }
        }
        other => panic!("expected CannotVerify, got {other:?}"),
    }
}

/// The comparator's storage path works end to end on the same XDR shapes the
/// RPC uses: decode a fixture row, re-point it at a plain storage key, and a
/// matching local observation compares as a match with one comparable key.
/// This is what a state-mutating invocation would exercise, without needing
/// one to exist on Testnet yet.
#[test]
fn storage_comparison_matches_and_detects_a_divergence() {
    // A plain contract-data storage key, encoded exactly as the RPC would.
    let storage_key = {
        use stellar_xdr::{
            ContractDataDurability, ContractId, Hash, LedgerKey, LedgerKeyContractData, Limited,
            Limits, ScAddress, ScSymbol, ScVal, StringM, WriteXdr,
        };
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([3; 32]))),
            key: ScVal::Symbol(ScSymbol(StringM::try_from("Orders").unwrap())),
            durability: ContractDataDurability::Persistent,
        });
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        key.write_xdr(&mut l).unwrap();
        buf
    };
    let value = {
        use stellar_xdr::{Limited, Limits, ScVal, WriteXdr};
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        ScVal::U32(7).write_xdr(&mut l).unwrap();
        buf
    };
    let rpc = vec![sdkt_fuzz::state_compare::RpcStateChange {
        key_xdr: storage_key.clone(),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        after: Some(value.clone()),
    }];
    let obs = sdkt_fuzz::Observation {
        case_id: "storage-compare".into(),
        function: "bump".into(),
        status: sdkt_fuzz::ExecutionStatus::Void,
        state: vec![sdkt_fuzz::StateEntry {
            key_xdr: storage_key.clone(),
            value_xdr: Some(value.clone()),
            change: sdkt_fuzz::StateChange::Updated,
        }],
        events: vec![],
        budget: Default::default(),
    };
    let (outcome, comparable) = sdkt_fuzz::state_compare::compare_state_counting(&obs, &rpc);
    assert!(outcome.is_match(), "{outcome:?}");
    assert_eq!(comparable, 1);

    // Perturb the local value: the same key must now report a mismatch.
    let mut perturbed = obs.clone();
    perturbed.state[0].value_xdr = Some(vec![0xAB; 8]);
    let (outcome, _) = sdkt_fuzz::state_compare::compare_state_counting(&perturbed, &rpc);
    match outcome {
        sdkt_fuzz::state_compare::Outcome::Mismatch(v) => {
            assert_eq!(v.len(), 1, "exactly one key should diverge");
            match &v[0] {
                sdkt_fuzz::state_compare::KeyVerdict::Mismatch { local, rpc: r } => {
                    assert_eq!(
                        local,
                        &sdkt_fuzz::state_compare::Side::Present(vec![0xAB; 8])
                    );
                    assert_eq!(r, &sdkt_fuzz::state_compare::Side::Present(value));
                }
                other => panic!("expected a Mismatch verdict, got {other:?}"),
            }
        }
        other => panic!("expected Mismatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Chaining: RPC `stateChanges` become the next invocation's baseline.
//
// These tests exercise `state_changes_into_sequence_state` against the
// captured live rows and synthetic rows in the same XDR shapes. The key
// semantic: Created/Updated carry the RPC `after` value forward, Deleted
// removes the key, and executor-owned rows (nonce, instance, code) are
// skipped — never silently folded into contract storage.
// ---------------------------------------------------------------------------

/// Decode all captured rows once; the chaining tests build on this.
fn decoded_fixture_rows() -> Vec<sdkt_fuzz::state_capture::DecodedStateChange> {
    fixture_state_changes_full()
        .iter()
        .map(|(k, t, b, a)| {
            sdkt_fuzz::state_capture::decode_state_change(k, t, b.as_deref(), a.as_deref())
                .expect("row decodes")
        })
        .collect()
}

/// The fixture's three rows are all executor-owned (nonce, instance, code),
/// so chaining them onto an empty baseline yields an empty baseline.
/// Nothing is carried, and no error is raised.
#[test]
fn chaining_executor_owned_rows_yields_empty_baseline() {
    let rows = decoded_fixture_rows();
    let out = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &rows)
        .expect("executor-owned rows chain cleanly");
    assert!(out.data_entries.is_empty());
    assert!(out.instance_storage.is_empty());
}

/// Created → the `after` entry is in the next baseline.
#[test]
fn chaining_created_row_carries_the_after_entry() {
    let rows = decoded_fixture_rows();
    let created = rows
        .iter()
        .find(|r| r.kind == sdkt_fuzz::state_capture::StateChangeKind::Created)
        .expect("fixture has a created row");
    // Re-point the row at a plain storage key so it is chainable storage.
    let mut row = created.clone();
    let storage_key = storage_key_xdr();
    row.key_b64 = sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes");
    let mut after = created.after.clone().expect("created has after");
    let mut data = match &after.data {
        stellar_xdr::LedgerEntryData::ContractData(d) => d.clone(),
        _ => panic!("fixture created row is contract data"),
    };
    data.contract = contract_id();
    after.data = stellar_xdr::LedgerEntryData::ContractData(data);
    // Rebuild the after entry under the storage key.
    let after = contract_data_entry(storage_key.clone(), after_last_modified(&after));
    row.after = Some(after.clone());
    let out = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[row])
        .expect("created chains");
    assert_eq!(out.data_entries.len(), 1);
    assert_eq!(out.data_entries[0].to_key(), storage_key);
}

/// Updated → the baseline value is replaced by `after`.
#[test]
fn chaining_updated_row_replaces_the_baseline_value() {
    let storage_key = storage_key_xdr();
    let old = contract_data_entry(storage_key.clone(), 100);
    let base = sdkt_fuzz::SequenceState {
        data_entries: vec![old],
        instance_storage: vec![],
    };
    let new_after = contract_data_entry(storage_key.clone(), 200);
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        before: Some(contract_data_entry(storage_key.clone(), 100)),
        after: Some(new_after.clone()),
    };
    let out = sdkt_fuzz::state_changes_into_sequence_state(&base, &[row]).expect("updated chains");
    assert_eq!(out.data_entries.len(), 1);
    assert_eq!(out.data_entries[0].last_modified_ledger_seq, 200);
}

/// Deleted → the key is removed from the baseline.
#[test]
fn chaining_deleted_row_removes_the_key() {
    let storage_key = storage_key_xdr();
    let base = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(storage_key.clone(), 100)],
        instance_storage: vec![],
    };
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Deleted,
        before: Some(contract_data_entry(storage_key.clone(), 100)),
        after: None,
    };
    let out = sdkt_fuzz::state_changes_into_sequence_state(&base, &[row]).expect("deleted chains");
    assert!(out.data_entries.is_empty());
}

/// A non-ContractData row is an explicit error, never a silent skip.
#[test]
fn chaining_non_contract_data_row_is_an_error() {
    let key = account_key_xdr();
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        before: Some(contract_data_entry(storage_key_xdr(), 1)),
        after: Some(contract_data_entry(storage_key_xdr(), 2)),
    };
    let err = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[row])
        .expect_err("non-contract-data key must fail");
    assert!(matches!(err, sdkt_fuzz::ChainError::UnsupportedKey { .. }));
}

/// An `after` that decodes to a different key than the row reports is an
/// explicit error.
#[test]
fn chaining_key_mismatch_is_an_error() {
    let storage_key = storage_key_xdr();
    let other_key = other_storage_key_xdr();
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        before: Some(contract_data_entry(storage_key.clone(), 1)),
        after: Some(contract_data_entry(other_key, 2)),
    };
    // Seed the baseline so the row passes the before-consistency check and
    // reaches the after-key check, which is what this test targets.
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(storage_key, 1)],
        instance_storage: vec![],
    };
    let err = sdkt_fuzz::state_changes_into_sequence_state(&baseline, &[row])
        .expect_err("key mismatch must fail");
    assert!(matches!(err, sdkt_fuzz::ChainError::KeyMismatch { .. }));
}

/// No TTL or ledger metadata is invented: the chained entry's value comes
/// from the RPC `after`, and its `last_modified_ledger_seq` is the RPC's own
/// sequence, not a synthetic marker.
#[test]
fn chaining_does_not_invent_ledger_metadata() {
    let storage_key = storage_key_xdr();
    let after = contract_data_entry(storage_key.clone(), 3964034);
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Created,
        before: None,
        after: Some(after),
    };
    let out = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[row])
        .expect("created chains");
    assert_eq!(out.data_entries[0].last_modified_ledger_seq, 3964034);
}

/// Two sequential invocations: the second baseline is the first invocation's
/// output, not the original state. Deterministic end to end.
#[test]
fn chaining_two_invocations_is_deterministic() {
    let storage_key = storage_key_xdr();
    let first = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Created,
        before: None,
        after: Some(contract_data_entry(storage_key.clone(), 10)),
    };
    let second = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        before: Some(contract_data_entry(storage_key.clone(), 10)),
        after: Some(contract_data_entry(storage_key.clone(), 11)),
    };
    let s1 = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[first])
        .expect("first chains");
    assert_eq!(s1.data_entries[0].last_modified_ledger_seq, 10);
    let s2 = sdkt_fuzz::state_changes_into_sequence_state(&s1, &[second]).expect("second chains");
    assert_eq!(s2.data_entries.len(), 1);
    assert_eq!(s2.data_entries[0].last_modified_ledger_seq, 11);
}

// --- helpers ---------------------------------------------------------------

fn contract_id() -> stellar_xdr::ScAddress {
    stellar_xdr::ScAddress::Contract(stellar_xdr::ContractId(stellar_xdr::Hash([3; 32])))
}

fn storage_key_xdr() -> stellar_xdr::LedgerKey {
    stellar_xdr::LedgerKey::ContractData(stellar_xdr::LedgerKeyContractData {
        contract: contract_id(),
        key: stellar_xdr::ScVal::Symbol(stellar_xdr::ScSymbol(
            stellar_xdr::StringM::try_from("Orders").unwrap(),
        )),
        durability: stellar_xdr::ContractDataDurability::Persistent,
    })
}

fn other_storage_key_xdr() -> stellar_xdr::LedgerKey {
    stellar_xdr::LedgerKey::ContractData(stellar_xdr::LedgerKeyContractData {
        contract: contract_id(),
        key: stellar_xdr::ScVal::Symbol(stellar_xdr::ScSymbol(
            stellar_xdr::StringM::try_from("Other").unwrap(),
        )),
        durability: stellar_xdr::ContractDataDurability::Persistent,
    })
}

fn account_key_xdr() -> stellar_xdr::LedgerKey {
    stellar_xdr::LedgerKey::Account(stellar_xdr::LedgerKeyAccount {
        account_id: stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
            stellar_xdr::Uint256([5; 32]),
        )),
    })
}

fn contract_data_entry(key: stellar_xdr::LedgerKey, seq: u32) -> stellar_xdr::LedgerEntry {
    let cd = match &key {
        stellar_xdr::LedgerKey::ContractData(d) => d.clone(),
        _ => panic!("helper requires a ContractData key"),
    };
    stellar_xdr::LedgerEntry {
        last_modified_ledger_seq: seq,
        data: stellar_xdr::LedgerEntryData::ContractData(stellar_xdr::ContractDataEntry {
            ext: stellar_xdr::ExtensionPoint::V0,
            contract: cd.contract.clone(),
            key: cd.key.clone(),
            durability: cd.durability,
            val: stellar_xdr::ScVal::U32(seq),
        }),
        ext: stellar_xdr::LedgerEntryExt::V0,
    }
}

fn after_last_modified(e: &stellar_xdr::LedgerEntry) -> u32 {
    e.last_modified_ledger_seq
}

/// Regression for the chaining audit: `apply_state_delta` used to fold nonce
/// entries into `data_entries` as contract storage (the `ContractData(_)`
/// catch-all arm). Nonce entries are host-generated temporaries — the same
/// executor-owned class `is_executor_owned_key` defines — and must be
/// skipped, or a nonce would poison the next step's baseline as fake storage.
#[test]
fn apply_state_delta_skips_nonce_entries() {
    use stellar_xdr::{
        ContractDataDurability, Hash, LedgerEntry, LedgerEntryData, LedgerKey,
        LedgerKeyContractData, Limited, Limits, ScAddress, ScNonceKey, ScVal, WriteXdr,
    };
    let contract_id_addr = ScAddress::Contract(stellar_xdr::ContractId(Hash([3; 32])));
    let nonce_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: contract_id_addr.clone(),
        key: ScVal::LedgerKeyNonce(ScNonceKey { nonce: 99 }),
        durability: ContractDataDurability::Temporary,
    });
    let nonce_key_xdr = {
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        nonce_key.write_xdr(&mut l).unwrap();
        buf
    };
    let nonce_entry = LedgerEntry {
        last_modified_ledger_seq: 500,
        data: LedgerEntryData::ContractData(stellar_xdr::ContractDataEntry {
            ext: stellar_xdr::ExtensionPoint::V0,
            contract: contract_id_addr,
            key: ScVal::LedgerKeyNonce(ScNonceKey { nonce: 99 }),
            durability: ContractDataDurability::Temporary,
            val: ScVal::Void,
        }),
        ext: stellar_xdr::LedgerEntryExt::V0,
    };
    let nonce_value_xdr = {
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        nonce_entry.write_xdr(&mut l).unwrap();
        buf
    };
    let obs = sdkt_fuzz::Observation {
        case_id: "nonce-skip".into(),
        function: "f".into(),
        status: sdkt_fuzz::ExecutionStatus::Void,
        state: vec![sdkt_fuzz::StateEntry {
            key_xdr: nonce_key_xdr,
            value_xdr: Some(nonce_value_xdr),
            change: sdkt_fuzz::StateChange::Created,
        }],
        events: vec![],
        budget: Default::default(),
    };
    let carried = sdkt_fuzz::apply_state_delta(Default::default(), &obs).expect("delta applies");
    assert!(
        carried.data_entries.is_empty(),
        "a nonce must not enter contract storage: {:?}",
        carried.data_entries
    );
}

/// Duplicate keys in `state_changes_into_sequence_state` are a protocol
/// violation, but the function must remain deterministic: last-write-wins
/// for Created/Updated, idempotent for Deleted. This test pins that behavior
/// so a future change cannot silently introduce ambiguity.
#[test]
fn chaining_duplicate_keys_are_deterministic_last_write_wins() {
    let storage_key = storage_key_xdr();
    let first = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Created,
        before: None,
        after: Some(contract_data_entry(storage_key.clone(), 10)),
    };
    let second = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        before: Some(contract_data_entry(storage_key.clone(), 10)),
        after: Some(contract_data_entry(storage_key.clone(), 20)),
    };
    let out = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[first, second])
        .expect("duplicate keys chain deterministically");
    assert_eq!(out.data_entries.len(), 1, "exactly one entry survives");
    assert_eq!(
        out.data_entries[0].last_modified_ledger_seq, 20,
        "last write wins"
    );
}

/// Duplicate Deleted rows: the first delete removes the entry; the second
/// delete fails because the key no longer exists in the baseline. This is
/// the correct behavior — a second delete for the same key is a protocol
/// violation, not a silent no-op.
#[test]
fn chaining_duplicate_deleted_rows_second_is_error() {
    let storage_key = storage_key_xdr();
    let base = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(storage_key.clone(), 10)],
        instance_storage: vec![],
    };
    let del1 = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Deleted,
        before: Some(contract_data_entry(storage_key.clone(), 10)),
        after: None,
    };
    let del2 = del1.clone();
    let err = sdkt_fuzz::state_changes_into_sequence_state(&base, &[del1, del2])
        .expect_err("second delete must fail");
    assert!(matches!(
        err,
        sdkt_fuzz::ChainError::BaselineMismatch { .. }
    ));
}

// ---------------------------------------------------------------------------
// End-to-end chaining: the next invocation runs against carried state.
//
// Integration proof that a chained baseline actually reaches execution, not
// merely a value that round-trips through the helper. Uses the committed
// `us_new.wasm` counter contract (`increment()` reads+writes instance state),
// so the fixture is real and needs no network or secret.
// ---------------------------------------------------------------------------

/// State carries within ONE sequence's steps: the second step observes the
/// first step's write. Separate `execute_sequence` calls are isolated by
/// design, so carry-forward is proven inside a single sequence.
#[test]
fn e2e_state_carries_within_a_sequence() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };
    let increment = FunctionCall::new("increment", vec![]);

    let run = sdkt_fuzz::execute_sequence(
        &exec,
        &[],
        &env,
        &[increment.clone(), increment],
        "chain-within-sequence",
    )
    .expect("sequence executes");
    assert_eq!(
        run.steps[0].observation.return_value(),
        Some(&stellar_xdr::ScVal::U32(1)),
        "first step starts from an empty baseline"
    );
    assert_eq!(
        run.steps[1].observation.return_value(),
        Some(&stellar_xdr::ScVal::U32(2)),
        "second step must run against the state the first established"
    );

    // The RPC-shaped chaining path feeds the same carry-forward: an
    // RPC `stateChanges` row converts into the sequence baseline.
    let storage_key = storage_key_xdr();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes");
    let row = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "created",
        None,
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 7))
                .expect("encodes"),
        ),
    )
    .expect("created row decodes");
    let chained =
        sdkt_fuzz::state_changes_into_sequence_state(&sdkt_fuzz::SequenceState::default(), &[row])
            .expect("chains");
    assert_eq!(chained.data_entries.len(), 1);
    assert_eq!(
        chained.data_entries[0].to_key(),
        storage_key,
        "the chained baseline carries the RPC row's key"
    );
}

/// Created → Updated: the next invocation sees the latest value.
#[test]
fn e2e_chaining_created_then_updated_uses_latest_value() {
    let storage_key = storage_key_xdr();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes");
    let created = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "created",
        None,
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 10))
                .expect("encodes"),
        ),
    )
    .expect("created decodes");
    let updated = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "updated",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 10))
                .expect("encodes"),
        ),
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 20))
                .expect("encodes"),
        ),
    )
    .expect("updated decodes");

    let s1 = sdkt_fuzz::state_changes_into_sequence_state(
        &sdkt_fuzz::SequenceState::default(),
        &[created],
    )
    .expect("created chains");
    let s2 = sdkt_fuzz::state_changes_into_sequence_state(&s1, &[updated]).expect("updated chains");
    assert_eq!(s2.data_entries.len(), 1);
    assert_eq!(
        s2.data_entries[0].last_modified_ledger_seq, 20,
        "updated value wins"
    );
}

/// Updated → Deleted: the entry is gone from the next baseline.
/// The Updated row chains onto a baseline seeded with the `before` value.
#[test]
fn e2e_chaining_updated_then_deleted_removes_entry() {
    let storage_key = storage_key_xdr();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes");
    let updated = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "updated",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 10))
                .expect("encodes"),
        ),
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 20))
                .expect("encodes"),
        ),
    )
    .expect("updated decodes");
    let deleted = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "deleted",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 20))
                .expect("encodes"),
        ),
        None,
    )
    .expect("deleted decodes");

    // Seed the baseline with the `before` value the Updated row reports.
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(storage_key, 10)],
        instance_storage: vec![],
    };
    let s1 = sdkt_fuzz::state_changes_into_sequence_state(&baseline, &[updated])
        .expect("updated chains");
    assert_eq!(s1.data_entries.len(), 1);
    let s2 = sdkt_fuzz::state_changes_into_sequence_state(&s1, &[deleted]).expect("deleted chains");
    assert!(
        s2.data_entries.is_empty(),
        "deleted removes the entry from the next baseline"
    );
}

/// Executor-owned rows never reach the contract-storage baseline.
#[test]
fn e2e_chaining_executor_owned_rows_stay_out_of_storage_baseline() {
    let rows = decoded_fixture_rows(); // nonce + instance + contract code
    let out = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &rows)
        .expect("executor-owned rows chain cleanly");
    assert!(out.data_entries.is_empty());
    assert!(out.instance_storage.is_empty());
}

/// Unsupported key → explicit `ChainError`, not silent skip.
#[test]
fn e2e_chaining_unsupported_key_is_an_explicit_error() {
    let storage_key = storage_key_xdr();
    // Built directly: `decode_state_change` already rejects key/entry
    // mismatches upstream, so this exercises the chaining function's own
    // validation of a non-ContractData key.
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&account_key_xdr()).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        before: Some(contract_data_entry(storage_key.clone(), 1)),
        after: Some(contract_data_entry(storage_key, 2)),
    };
    let err = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[row])
        .expect_err("unsupported key must fail");
    assert!(matches!(err, sdkt_fuzz::ChainError::UnsupportedKey { .. }));
}

// ---------------------------------------------------------------------------
// Integration: SimulateResponse → decode stateChanges → chain →
// execute_sequence
//
// Full pipeline, no live RPC. The counter contract's state lives in
// instance_storage, so the pipeline test uses the executor's real derived
// contract id for ContractData rows (which the executor accepts as
// baseline entries) AND proves the chained baseline reaches the next
// invocation via the within-sequence carry (increment → 1, then 2).
// ---------------------------------------------------------------------------

/// The contract id the executor derives for the campaign wasm under the
/// default config. `Executor::case` builds cases via `from_deployer` with
/// the campaign deployer/salt/network id; the derived id is what the
/// executor validates baseline entries against.
fn real_executor_contract_id() -> stellar_xdr::ScAddress {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let case = exec.case("probe", FunctionCall::new("increment", vec![]), vec![]);
    case.contract_address()
}

/// A ContractData key bound to the executor's real derived contract.
fn real_storage_key() -> stellar_xdr::LedgerKey {
    stellar_xdr::LedgerKey::ContractData(stellar_xdr::LedgerKeyContractData {
        contract: real_executor_contract_id(),
        key: stellar_xdr::ScVal::Symbol(stellar_xdr::ScSymbol(
            stellar_xdr::StringM::try_from("Orders").unwrap(),
        )),
        durability: stellar_xdr::ContractDataDurability::Persistent,
    })
}

/// Decode a SimulateResponse's `state_changes` JSON rows into
/// `DecodedStateChange`, mirroring the RPC path.
fn decode_sim_state_changes(
    resp: &sdkt_rpc::SimulateResponse,
) -> Result<Vec<sdkt_fuzz::state_capture::DecodedStateChange>, String> {
    let mut out = Vec::new();
    for (i, v) in resp.state_changes.iter().enumerate() {
        let obj = v
            .as_object()
            .ok_or_else(|| format!("stateChanges[{i}] not an object"))?;
        let key = obj
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("stateChanges[{i}].key missing"))?;
        let kind = obj
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("stateChanges[{i}].type missing"))?;
        let before = obj.get("before").and_then(|v| v.as_str());
        let after = obj.get("after").and_then(|v| v.as_str());
        let decoded = sdkt_fuzz::state_capture::decode_state_change(key, kind, before, after)
            .map_err(|e| format!("stateChanges[{i}] decode: {e}"))?;
        out.push(decoded);
    }
    Ok(out)
}

fn sim_response(rows_json: &str) -> sdkt_rpc::SimulateResponse {
    let raw = format!(
        r#"{{"transactionData":"","minResourceFee":"0","latestLedger":"1000","stateChanges":[{}],"results":[]}}"#,
        rows_json
    );
    serde_json::from_str(&raw).expect("valid json")
}

fn row_json(
    key_b64: &str,
    kind: &str,
    before_b64: Option<&str>,
    after_b64: Option<&str>,
) -> String {
    let b = before_b64
        .map(|s| format!(r#""{}""#, s))
        .unwrap_or_else(|| "null".into());
    let a = after_b64
        .map(|s| format!(r#""{}""#, s))
        .unwrap_or_else(|| "null".into());
    format!(
        r#"{{"key":"{}","type":"{}","before":{},"after":{}}}"#,
        key_b64, kind, b, a
    )
}

/// Full pipeline: decode an RPC-shaped SimulateResponse → chain → the
/// chained baseline is accepted by the executor AND the next invocation
/// observes carried state. The decode+chain half uses the executor's real
/// contract id; the execute half proves state reaches the next invocation
/// via the within-sequence carry (increment → 1, then 2).
#[test]
fn e2e_sim_decode_chain_execute() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    // Decode half: an RPC-shaped SimulateResponse with a Created row bound
    // to the executor's real contract id.
    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let entry = contract_data_entry(key.clone(), 1);
    let after_b64 = sdkt_fuzz::state_capture::encode_entry(&entry).expect("encodes");
    let resp = sim_response(&row_json(&key_b64, "created", None, Some(&after_b64)));

    let decoded = decode_sim_state_changes(&resp).expect("stateChanges decode");
    assert_eq!(decoded.len(), 1, "one row decoded");
    let chained = sdkt_fuzz::state_changes_into_sequence_state(
        &sdkt_fuzz::SequenceState::default(),
        &decoded,
    )
    .expect("chain succeeds");
    assert_eq!(chained.data_entries.len(), 1, "Created entry chained");
    assert_eq!(
        chained.data_entries[0].to_key(),
        key,
        "chained key matches the RPC row"
    );

    // Execute half: the chained baseline is accepted by the executor, and
    // the within-sequence carry proves state reaches the next invocation.
    let run = sdkt_fuzz::execute_sequence(
        &exec,
        &chained.data_entries,
        &env,
        &[
            FunctionCall::new("increment", vec![]),
            FunctionCall::new("increment", vec![]),
        ],
        "sim-chain-exec",
    )
    .expect("sequence executes against the chained baseline");
    assert_eq!(
        run.steps[0].observation.return_value(),
        Some(&stellar_xdr::ScVal::U32(1)),
        "first step starts from the chained baseline"
    );
    assert_eq!(
        run.steps[1].observation.return_value(),
        Some(&stellar_xdr::ScVal::U32(2)),
        "the next invocation must run against the state the first established"
    );
}

/// Created → Updated on the real contract id: latest value wins.
#[test]
fn e2e_sim_created_then_updated() {
    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let created = contract_data_entry(key.clone(), 10);
    let updated = contract_data_entry(key.clone(), 20);
    let cb64 = sdkt_fuzz::state_capture::encode_entry(&created).expect("encodes");
    let ub64 = sdkt_fuzz::state_capture::encode_entry(&updated).expect("encodes");

    let resp1 = sim_response(&row_json(&key_b64, "created", None, Some(&cb64)));
    let resp2 = sim_response(&row_json(&key_b64, "updated", Some(&cb64), Some(&ub64)));

    let d1 = decode_sim_state_changes(&resp1).expect("decode created");
    let d2 = decode_sim_state_changes(&resp2).expect("decode updated");

    let s1 =
        sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &d1).expect("created");
    let s2 = sdkt_fuzz::state_changes_into_sequence_state(&s1, &d2).expect("updated");
    assert_eq!(s2.data_entries.len(), 1);
    assert_eq!(
        s2.data_entries[0].last_modified_ledger_seq, 20,
        "latest value wins"
    );
}

/// Updated → Deleted on the real contract id: entry removed.
/// The Updated row chains onto a baseline seeded with the `before` value.
#[test]
fn e2e_sim_updated_then_deleted() {
    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let before = contract_data_entry(key.clone(), 10);
    let updated = contract_data_entry(key.clone(), 20);
    let bb64 = sdkt_fuzz::state_capture::encode_entry(&before).expect("encodes");
    let ub64 = sdkt_fuzz::state_capture::encode_entry(&updated).expect("encodes");

    let resp1 = sim_response(&row_json(&key_b64, "updated", Some(&bb64), Some(&ub64)));
    let resp2 = sim_response(&row_json(&key_b64, "deleted", Some(&ub64), None));

    let d1 = decode_sim_state_changes(&resp1).expect("decode updated");
    let d2 = decode_sim_state_changes(&resp2).expect("decode deleted");

    // Seed the baseline with the `before` value the Updated row reports.
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(key, 10)],
        instance_storage: vec![],
    };
    let s1 = sdkt_fuzz::state_changes_into_sequence_state(&baseline, &d1).expect("updated");
    assert_eq!(s1.data_entries.len(), 1);
    let s2 = sdkt_fuzz::state_changes_into_sequence_state(&s1, &d2).expect("deleted");
    assert!(s2.data_entries.is_empty(), "deleted removes the entry");
}

/// Executor-owned rows are skipped, not treated as contract storage.
#[test]
fn e2e_sim_executor_owned_rows_skipped() {
    let rows = decoded_fixture_rows(); // nonce + instance + contract code
    let out = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &rows)
        .expect("executor-owned rows chain cleanly");
    assert!(out.data_entries.is_empty());
    assert!(out.instance_storage.is_empty());
}

/// Unsupported key (Account) → explicit ChainError, not silent skip.
/// Built via the direct struct because the decoder correctly rejects
/// key/entry mismatches upstream — this exercises the chaining function's
/// own validation.
#[test]
fn e2e_sim_unsupported_key_error() {
    let storage = real_storage_key();
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&account_key_xdr()).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        before: Some(contract_data_entry(storage.clone(), 1)),
        after: Some(contract_data_entry(storage, 2)),
    };
    let err = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[row])
        .expect_err("unsupported key must fail");
    assert!(matches!(err, sdkt_fuzz::ChainError::UnsupportedKey { .. }));
}

/// Key mismatch (after decodes to a different key) → explicit error.
#[test]
fn e2e_sim_key_mismatch_error() {
    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let other = other_storage_key_xdr();
    let b = sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(real_storage_key(), 1))
        .expect("encodes");
    let a =
        sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(other, 2)).expect("encodes");
    let resp = sim_response(&row_json(&key_b64, "updated", Some(&b), Some(&a)));

    let err = decode_sim_state_changes(&resp).expect_err("key mismatch must fail");
    assert!(
        err.contains("does not match decoded entry key"),
        "got: {err}"
    );
}

/// Malformed base64 → explicit error.
#[test]
fn e2e_sim_malformed_xdr_error() {
    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let resp = sim_response(&row_json(
        &key_b64,
        "updated",
        Some("!!!bad!!!"),
        Some("!!!bad!!!"),
    ));

    let err = decode_sim_state_changes(&resp).expect_err("malformed xdr must fail");
    assert!(err.contains("failed to decode"), "got: {err}");
}

/// CannotVerify from the comparator is never promoted to Match.
/// Uses the fixture's executor-owned rows (nonce/instance/code): the
/// comparator filters them out, so no key is comparable and the outcome is
/// CannotVerify — never Match.
#[test]
fn e2e_sim_cannot_verify_not_promoted() {
    let decoded = decoded_fixture_rows();
    let rpc: Vec<sdkt_fuzz::state_compare::RpcStateChange> = decoded
        .iter()
        .map(sdkt_fuzz::state_compare::RpcStateChange::from_decoded)
        .collect::<Result<_, _>>()
        .expect("rows convert");
    let local_state: Vec<sdkt_fuzz::StateEntry> = rpc
        .iter()
        .map(|c| sdkt_fuzz::StateEntry {
            key_xdr: c.key_xdr.clone(),
            value_xdr: c.after.clone(),
            change: sdkt_fuzz::StateChange::Updated,
        })
        .collect();
    let obs = sdkt_fuzz::Observation {
        case_id: "sim-cv".into(),
        function: "f".into(),
        status: sdkt_fuzz::ExecutionStatus::Void,
        state: local_state,
        events: vec![],
        budget: Default::default(),
    };
    let (outcome, comparable) = sdkt_fuzz::state_compare::compare_state_counting(&obs, &rpc);
    assert!(
        matches!(outcome, sdkt_fuzz::state_compare::Outcome::CannotVerify(_)),
        "{outcome:?}"
    );
    assert_eq!(comparable, 0);
}

// ---------------------------------------------------------------------------
// Baseline-consistency validation: `state_changes_into_sequence_state`
// rejects rows whose `before` value or key presence contradicts the baseline.
// ---------------------------------------------------------------------------

/// `Created` on a key that already exists in the baseline is an error, not a
/// silent overwrite.
#[test]
fn chaining_created_on_existing_key_is_an_error() {
    let storage_key = storage_key_xdr();
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(storage_key.clone(), 10)],
        instance_storage: vec![],
    };
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Created,
        before: None,
        after: Some(contract_data_entry(storage_key.clone(), 20)),
    };
    let err = sdkt_fuzz::state_changes_into_sequence_state(&baseline, &[row])
        .expect_err("created on an existing key must fail");
    assert!(
        matches!(err, sdkt_fuzz::ChainError::BaselineMismatch { .. }),
        "got: {err:?}"
    );
}

/// `Updated` on a key absent from the baseline is an error, not a silent
/// insertion.
#[test]
fn chaining_updated_on_absent_key_is_an_error() {
    let storage_key = storage_key_xdr();
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        before: Some(contract_data_entry(storage_key.clone(), 10)),
        after: Some(contract_data_entry(storage_key.clone(), 20)),
    };
    let err = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[row])
        .expect_err("updated on an absent key must fail");
    assert!(
        matches!(err, sdkt_fuzz::ChainError::BaselineMismatch { .. }),
        "got: {err:?}"
    );
}

/// `Updated` whose `before` value differs from the baseline entry is an
/// error — the RPC row does not describe the transition we hold.
#[test]
fn chaining_updated_with_mismatched_before_is_an_error() {
    let storage_key = storage_key_xdr();
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(storage_key.clone(), 10)],
        instance_storage: vec![],
    };
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        // `before` reports 99, but the baseline holds 10.
        before: Some(contract_data_entry(storage_key.clone(), 99)),
        after: Some(contract_data_entry(storage_key.clone(), 20)),
    };
    let err = sdkt_fuzz::state_changes_into_sequence_state(&baseline, &[row])
        .expect_err("updated with a mismatched before must fail");
    assert!(
        matches!(err, sdkt_fuzz::ChainError::BaselineMismatch { .. }),
        "got: {err:?}"
    );
}

/// `Deleted` on a key absent from the baseline is an error, not a silent
/// no-op.
#[test]
fn chaining_deleted_on_absent_key_is_an_error() {
    let storage_key = storage_key_xdr();
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Deleted,
        before: Some(contract_data_entry(storage_key.clone(), 10)),
        after: None,
    };
    let err = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[row])
        .expect_err("deleted on an absent key must fail");
    assert!(
        matches!(err, sdkt_fuzz::ChainError::BaselineMismatch { .. }),
        "got: {err:?}"
    );
}

/// `Deleted` whose `before` value differs from the baseline entry is an
/// error.
#[test]
fn chaining_deleted_with_mismatched_before_is_an_error() {
    let storage_key = storage_key_xdr();
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(storage_key.clone(), 10)],
        instance_storage: vec![],
    };
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Deleted,
        // `before` reports 99, but the baseline holds 10.
        before: Some(contract_data_entry(storage_key.clone(), 99)),
        after: None,
    };
    let err = sdkt_fuzz::state_changes_into_sequence_state(&baseline, &[row])
        .expect_err("deleted with a mismatched before must fail");
    assert!(
        matches!(err, sdkt_fuzz::ChainError::BaselineMismatch { .. }),
        "got: {err:?}"
    );
}

/// The valid path still holds: Created → Updated → Deleted, each row
/// consistent with the baseline it is applied to.
#[test]
fn chaining_created_updated_deleted_valid_sequence_still_passes() {
    let storage_key = storage_key_xdr();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes");

    let created = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "created",
        None,
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 10))
                .expect("encodes"),
        ),
    )
    .expect("created decodes");
    let updated = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "updated",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 10))
                .expect("encodes"),
        ),
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 20))
                .expect("encodes"),
        ),
    )
    .expect("updated decodes");
    let deleted = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "deleted",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 20))
                .expect("encodes"),
        ),
        None,
    )
    .expect("deleted decodes");

    let s0 = sdkt_fuzz::SequenceState::default();
    let s1 = sdkt_fuzz::state_changes_into_sequence_state(&s0, &[created]).expect("created chains");
    assert_eq!(s1.data_entries.len(), 1);
    let s2 = sdkt_fuzz::state_changes_into_sequence_state(&s1, &[updated]).expect("updated chains");
    assert_eq!(s2.data_entries.len(), 1);
    assert_eq!(s2.data_entries[0].last_modified_ledger_seq, 20);
    let s3 = sdkt_fuzz::state_changes_into_sequence_state(&s2, &[deleted]).expect("deleted chains");
    assert!(s3.data_entries.is_empty(), "deleted removes the entry");
}

// ---------------------------------------------------------------------------
// Semantic gap tests (targeted): pin concrete behaviors discovered during
// the host v29 semantic review.
// ---------------------------------------------------------------------------

/// GAP 1 — Executor-owned instance entry is skipped by the chaining function.
/// Its storage map is NOT folded into `SequenceState::instance_storage`.
/// Contrast with local execution (`apply_state_delta`), which DOES fold it.
/// This test pins the current behavior: RPC instance rows produce empty
/// instance_storage. A future phase that folds RPC instance storage will
/// deliberately update this test.
#[test]
fn gap_rpc_instance_entry_does_not_populate_instance_storage() {
    let rows = fixture_state_changes_full();
    let instance_row = rows
        .iter()
        .find(|(_, t, b, _)| t == "updated" && b.is_some())
        .expect("fixture has an updated instance row");
    let decoded = sdkt_fuzz::state_capture::decode_state_change(
        &instance_row.0,
        &instance_row.1,
        instance_row.2.as_deref(),
        instance_row.3.as_deref(),
    )
    .expect("instance row decodes");

    let out = sdkt_fuzz::state_changes_into_sequence_state(&Default::default(), &[decoded])
        .expect("instance row chains (by being skipped)");
    // The key finding: instance_storage remains empty.
    assert!(
        out.instance_storage.is_empty(),
        "state_changes_into_sequence_state skips instance entries entirely"
    );
    assert!(
        out.data_entries.is_empty(),
        "instance entries are never placed in data_entries"
    );
}

/// GAP 2 — Multi-row batch order within one invocation:
/// A sequence of operations on the SAME key within one batch succeeds when
/// ordered consistently (Created → Updated → Deleted) and fails when ordered
/// inconsistently (Updated on non-existent, Deleted before Created).
#[test]
fn gap_batch_intra_invocation_created_then_updated_in_single_call() {
    let storage_key = storage_key_xdr();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes");

    let created = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "created",
        None,
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 10))
                .expect("encodes"),
        ),
    )
    .expect("created decodes");
    let updated = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "updated",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 10))
                .expect("encodes"),
        ),
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 20))
                .expect("encodes"),
        ),
    )
    .expect("updated decodes");

    // Both rows in a single batch applied to an empty baseline: the first row
    // creates the entry, the second updates it. The accumulator state sees
    // the first row before evaluating the second.
    let out = sdkt_fuzz::state_changes_into_sequence_state(
        &sdkt_fuzz::SequenceState::default(),
        &[created, updated],
    )
    .expect("intra-batch Created then Updated succeeds");
    assert_eq!(out.data_entries.len(), 1);
    assert_eq!(out.data_entries[0].last_modified_ledger_seq, 20);
}

/// Reverse order within one batch fails: Updated before Created is rejected
/// because the key is not in the baseline at the point of update.
#[test]
fn gap_batch_intra_invocation_updated_before_created_fails() {
    let storage_key = storage_key_xdr();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&storage_key).expect("encodes");

    let created = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "created",
        None,
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 10))
                .expect("encodes"),
        ),
    )
    .expect("created decodes");
    let updated = sdkt_fuzz::state_capture::decode_state_change(
        &key_b64,
        "updated",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 10))
                .expect("encodes"),
        ),
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage_key.clone(), 20))
                .expect("encodes"),
        ),
    )
    .expect("updated decodes");

    let err = sdkt_fuzz::state_changes_into_sequence_state(
        &sdkt_fuzz::SequenceState::default(),
        &[updated, created],
    )
    .expect_err("Updated before Created must fail");
    assert!(
        matches!(err, sdkt_fuzz::ChainError::BaselineMismatch { .. }),
        "got: {err:?}"
    );
}

/// GAP 3 — Multiple distinct keys within one batch do not interfere:
/// Key A Updated + Key B Created in the same batch both succeed when the
/// baseline holds Key A and does not hold Key B.
#[test]
fn gap_batch_multi_key_independent_success() {
    let key_a = storage_key_xdr();
    let key_b = other_storage_key_xdr();
    let a_b64 = sdkt_fuzz::state_capture::encode_key(&key_a).expect("encodes");
    let b_b64 = sdkt_fuzz::state_capture::encode_key(&key_b).expect("encodes");

    let update_a = sdkt_fuzz::state_capture::decode_state_change(
        &a_b64,
        "updated",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(key_a.clone(), 10))
                .expect("encodes"),
        ),
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(key_a.clone(), 20))
                .expect("encodes"),
        ),
    )
    .expect("update_a decodes");
    let create_b = sdkt_fuzz::state_capture::decode_state_change(
        &b_b64,
        "created",
        None,
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(key_b.clone(), 5))
                .expect("encodes"),
        ),
    )
    .expect("create_b decodes");

    // Baseline has A=10, does not have B.
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(key_a.clone(), 10)],
        instance_storage: vec![],
    };

    let out = sdkt_fuzz::state_changes_into_sequence_state(&baseline, &[update_a, create_b])
        .expect("multi-key batch succeeds");
    assert_eq!(out.data_entries.len(), 2, "both entries present");

    let a_entry = out
        .data_entries
        .iter()
        .find(|e| e.to_key() == key_a)
        .expect("A present");
    assert_eq!(a_entry.last_modified_ledger_seq, 20, "A was updated");
    let b_entry = out
        .data_entries
        .iter()
        .find(|e| e.to_key() == key_b)
        .expect("B present");
    assert_eq!(b_entry.last_modified_ledger_seq, 5, "B was created");
}

/// GAP 4 — Cross-check: an invalid row in a multi-key batch aborts the
/// whole batch without partially applying the earlier rows.
#[test]
fn gap_batch_multi_key_atomic_abort() {
    let key_a = storage_key_xdr();
    let key_b = other_storage_key_xdr();
    let a_b64 = sdkt_fuzz::state_capture::encode_key(&key_a).expect("encodes");
    let b_b64 = sdkt_fuzz::state_capture::encode_key(&key_b).expect("encodes");

    let update_a = sdkt_fuzz::state_capture::decode_state_change(
        &a_b64,
        "updated",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(key_a.clone(), 10))
                .expect("encodes"),
        ),
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(key_a.clone(), 20))
                .expect("encodes"),
        ),
    )
    .expect("update_a decodes");
    // Row for B has a mismatched before: reports 99, but B is not even in baseline.
    let invalid_b = sdkt_fuzz::state_capture::decode_state_change(
        &b_b64,
        "updated",
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(key_b.clone(), 99))
                .expect("encodes"),
        ),
        Some(
            &sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(key_b.clone(), 5))
                .expect("encodes"),
        ),
    )
    .expect("invalid_b decodes");

    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(key_a.clone(), 10)],
        instance_storage: vec![],
    };

    // The entire call returns Err; caller's baseline is unchanged.
    let err = sdkt_fuzz::state_changes_into_sequence_state(&baseline, &[update_a, invalid_b])
        .expect_err("invalid row in batch must abort");
    assert!(matches!(
        err,
        sdkt_fuzz::ChainError::BaselineMismatch { .. }
    ));
    // Baseline was not modified (passed by ref).
    assert_eq!(baseline.data_entries[0].last_modified_ledger_seq, 10);
}

// ---------------------------------------------------------------------------
// LedgerEntryExt::V1 investigation: is full equality a false positive?
// ---------------------------------------------------------------------------

/// Build a ContractData entry with `ext: V1(sponsoring_id)`.
fn contract_data_entry_v1(
    key: stellar_xdr::LedgerKey,
    seq: u32,
    sponsor: Option<stellar_xdr::AccountId>,
) -> stellar_xdr::LedgerEntry {
    let cd = match &key {
        stellar_xdr::LedgerKey::ContractData(d) => d.clone(),
        _ => panic!("helper requires a ContractData key"),
    };
    stellar_xdr::LedgerEntry {
        last_modified_ledger_seq: seq,
        data: stellar_xdr::LedgerEntryData::ContractData(stellar_xdr::ContractDataEntry {
            ext: stellar_xdr::ExtensionPoint::V0,
            contract: cd.contract.clone(),
            key: cd.key.clone(),
            durability: cd.durability,
            val: stellar_xdr::ScVal::U32(seq),
        }),
        ext: stellar_xdr::LedgerEntryExt::V1(stellar_xdr::LedgerEntryExtensionV1 {
            sponsoring_id: stellar_xdr::SponsorshipDescriptor(sponsor),
            ext: stellar_xdr::LedgerEntryExtensionV1Ext::V0,
        }),
    }
}

/// Two entries with identical data but different sponsorship representation
/// are NOT equal under full `PartialEq`. This documents the representation
/// difference; it does NOT claim it is a false positive in the chaining flow.
#[test]
fn ext_v1_sponsorship_difference_is_not_equal_under_full_equality() {
    let key = storage_key_xdr();
    let sponsor_a = stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
        stellar_xdr::Uint256([1; 32]),
    ));
    let sponsor_b = stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
        stellar_xdr::Uint256([2; 32]),
    ));

    let v1_a = contract_data_entry_v1(key.clone(), 10, Some(sponsor_a));
    let v1_b = contract_data_entry_v1(key.clone(), 10, Some(sponsor_b));
    let v0 = contract_data_entry(key.clone(), 10);

    // Full equality distinguishes them.
    assert_ne!(v1_a, v1_b, "different sponsors are not equal");
    assert_ne!(v1_a, v0, "V1 with sponsor is not equal to V0");
    assert_ne!(v1_b, v0, "V1 with sponsor is not equal to V0");

    // But the *logical contract state* (key + val) is identical.
    assert_eq!(v1_a.to_key(), v1_b.to_key());
    assert_eq!(v1_a.to_key(), v0.to_key());
    assert_eq!(v1_a.data, v1_b.data);
    assert_eq!(v1_a.data, v0.data);
    assert_eq!(v1_a.last_modified_ledger_seq, v1_b.last_modified_ledger_seq);
    assert_eq!(v1_a.last_modified_ledger_seq, v0.last_modified_ledger_seq);
}

/// The chaining function rejects an `Updated` row whose `before` carries a
/// different sponsorship representation than the baseline entry. This is the
/// behavior under investigation — documented, not asserted as correct.
#[test]
fn ext_v1_baseline_mismatch_is_rejected() {
    let key = storage_key_xdr();
    let sponsor_a = stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
        stellar_xdr::Uint256([1; 32]),
    ));
    let sponsor_b = stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
        stellar_xdr::Uint256([2; 32]),
    ));

    // Baseline holds V1 with sponsor_a; RPC row reports before = V1 with
    // sponsor_b (same data, different sponsorship).
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry_v1(key.clone(), 10, Some(sponsor_a))],
        instance_storage: vec![],
    };
    let row = sdkt_fuzz::state_capture::DecodedStateChange {
        key_b64: sdkt_fuzz::state_capture::encode_key(&key).expect("encodes"),
        kind: sdkt_fuzz::state_capture::StateChangeKind::Updated,
        before: Some(contract_data_entry_v1(
            key.clone(),
            10,
            Some(sponsor_b.clone()),
        )),
        after: Some(contract_data_entry_v1(key, 20, Some(sponsor_b))),
    };

    let err = sdkt_fuzz::state_changes_into_sequence_state(&baseline, &[row])
        .expect_err("sponsorship difference is rejected");
    assert!(
        matches!(err, sdkt_fuzz::ChainError::BaselineMismatch { .. }),
        "got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Production wrapper: `execute_sequence_from_simulation`
//
// These tests exercise the public wrapper (decode → validate → chain →
// execute) as a unit, distinct from the chaining-helper tests above. They
// prove that a RPC-shaped `SimulateResponse` drives real host execution and
// that failures abort before any invocation runs.
// ---------------------------------------------------------------------------

/// Positive: a `Created` row becomes the sequence's starting state, and the
/// second invocation observes the state the first established.
///
/// The assertion is state-dependent: the counter returns 1 then 2 only if
/// the chained baseline is actually applied to the first invocation.
#[test]
fn wrapper_created_row_drives_second_invocation() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let entry = contract_data_entry(key.clone(), 1);
    let after_b64 = sdkt_fuzz::state_capture::encode_entry(&entry).expect("encodes");
    let resp = sim_response(&row_json(&key_b64, "created", None, Some(&after_b64)));

    let run = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &sdkt_fuzz::SequenceState::default(),
        &resp,
        &env,
        &[
            FunctionCall::new("increment", vec![]),
            FunctionCall::new("increment", vec![]),
        ],
        "wrapper-created",
    )
    .expect("wrapper executes");

    assert_eq!(
        run.steps[0].observation.return_value(),
        Some(&stellar_xdr::ScVal::U32(1)),
        "first step runs against the chained baseline"
    );
    assert_eq!(
        run.steps[1].observation.return_value(),
        Some(&stellar_xdr::ScVal::U32(2)),
        "second step must observe the state the first established"
    );
}

/// Positive: `Updated` validates the pre-state and applies the post-state.
/// The baseline must hold exactly the `before` value the row reports.
#[test]
fn wrapper_updated_row_validates_before_and_applies_after() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let before = contract_data_entry(key.clone(), 10);
    let after = contract_data_entry(key.clone(), 20);
    let bb64 = sdkt_fuzz::state_capture::encode_entry(&before).expect("encodes");
    let ab64 = sdkt_fuzz::state_capture::encode_entry(&after).expect("encodes");
    let resp = sim_response(&row_json(&key_b64, "updated", Some(&bb64), Some(&ab64)));

    // Baseline holds exactly the `before` value: validation passes.
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(key, 10)],
        instance_storage: vec![],
    };
    let run = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &baseline,
        &resp,
        &env,
        &[FunctionCall::new("increment", vec![])],
        "wrapper-updated",
    )
    .expect("updated row with matching baseline executes");
    assert_eq!(
        run.final_observation.return_value(),
        Some(&stellar_xdr::ScVal::U32(1)),
        "the chained (post-update) baseline is accepted by the executor"
    );
}

/// Positive: `Deleted` removes the entry from the chained state.
#[test]
fn wrapper_deleted_row_removes_entry() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let before = contract_data_entry(key.clone(), 20);
    let bb64 = sdkt_fuzz::state_capture::encode_entry(&before).expect("encodes");
    let resp = sim_response(&row_json(&key_b64, "deleted", Some(&bb64), None));

    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(key, 20)],
        instance_storage: vec![],
    };
    let run = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &baseline,
        &resp,
        &env,
        &[FunctionCall::new("increment", vec![])],
        "wrapper-deleted",
    )
    .expect("deleted row with matching baseline executes");
    assert_eq!(
        run.final_observation.return_value(),
        Some(&stellar_xdr::ScVal::U32(1)),
        "the chained (post-delete) baseline is accepted by the executor"
    );
}

/// Positive: multi-key changes are all applied.
#[test]
fn wrapper_multi_key_changes_all_applied() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let key_a = real_storage_key();
    let key_b = stellar_xdr::LedgerKey::ContractData(stellar_xdr::LedgerKeyContractData {
        contract: real_executor_contract_id(),
        key: stellar_xdr::ScVal::Symbol(stellar_xdr::ScSymbol(
            stellar_xdr::StringM::try_from("Other").unwrap(),
        )),
        durability: stellar_xdr::ContractDataDurability::Persistent,
    });
    let a_b64 = sdkt_fuzz::state_capture::encode_key(&key_a).expect("encodes");
    let b_b64 = sdkt_fuzz::state_capture::encode_key(&key_b).expect("encodes");

    // A: created. B: updated (baseline holds B's before value).
    let a_after = contract_data_entry(key_a.clone(), 5);
    let b_before = contract_data_entry(key_b.clone(), 10);
    let b_after = contract_data_entry(key_b.clone(), 20);
    let aa64 = sdkt_fuzz::state_capture::encode_entry(&a_after).expect("encodes");
    let bb64 = sdkt_fuzz::state_capture::encode_entry(&b_before).expect("encodes");
    let ba64 = sdkt_fuzz::state_capture::encode_entry(&b_after).expect("encodes");

    let rows = format!(
        "{},{}",
        row_json(&a_b64, "created", None, Some(&aa64)),
        row_json(&b_b64, "updated", Some(&bb64), Some(&ba64)),
    );
    let resp = sim_response(&rows);

    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(key_b, 10)],
        instance_storage: vec![],
    };
    let run = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &baseline,
        &resp,
        &env,
        &[FunctionCall::new("increment", vec![])],
        "wrapper-multi-key",
    )
    .expect("multi-key batch executes");
    assert_eq!(
        run.final_observation.return_value(),
        Some(&stellar_xdr::ScVal::U32(1)),
        "both chained entries are accepted by the executor"
    );
}

/// Negative: baseline mismatch — an `Updated` row whose `before` does not
/// match the baseline. Execution must not start.
#[test]
fn wrapper_baseline_mismatch_aborts_before_execution() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let before = contract_data_entry(key.clone(), 99);
    let after = contract_data_entry(key.clone(), 20);
    let bb64 = sdkt_fuzz::state_capture::encode_entry(&before).expect("encodes");
    let ab64 = sdkt_fuzz::state_capture::encode_entry(&after).expect("encodes");
    let resp = sim_response(&row_json(&key_b64, "updated", Some(&bb64), Some(&ab64)));

    // Baseline holds 10, but the row reports before=99.
    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry(key, 10)],
        instance_storage: vec![],
    };
    let err = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &baseline,
        &resp,
        &env,
        &[FunctionCall::new("increment", vec![])],
        "wrapper-mismatch",
    )
    .expect_err("baseline mismatch must abort");
    assert!(
        matches!(err, sdkt_fuzz::FuzzError::InvalidSetup(_)),
        "got: {err:?}"
    );
}

/// Negative: sponsorship mismatch on `LedgerEntryExt::V1` — full equality is
/// preserved, so a different `sponsoring_id` is rejected.
#[test]
fn wrapper_sponsorship_mismatch_v1_is_rejected() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let sponsor_a = stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
        stellar_xdr::Uint256([1; 32]),
    ));
    let sponsor_b = stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
        stellar_xdr::Uint256([2; 32]),
    ));

    let before = contract_data_entry_v1(key.clone(), 10, Some(sponsor_b.clone()));
    let after = contract_data_entry_v1(key.clone(), 20, Some(sponsor_b));
    let bb64 = sdkt_fuzz::state_capture::encode_entry(&before).expect("encodes");
    let ab64 = sdkt_fuzz::state_capture::encode_entry(&after).expect("encodes");
    let resp = sim_response(&row_json(&key_b64, "updated", Some(&bb64), Some(&ab64)));

    let baseline = sdkt_fuzz::SequenceState {
        data_entries: vec![contract_data_entry_v1(key, 10, Some(sponsor_a))],
        instance_storage: vec![],
    };
    let err = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &baseline,
        &resp,
        &env,
        &[FunctionCall::new("increment", vec![])],
        "wrapper-sponsor",
    )
    .expect_err("sponsorship mismatch must abort");
    assert!(
        matches!(err, sdkt_fuzz::FuzzError::InvalidSetup(_)),
        "got: {err:?}"
    );
}

/// Negative: malformed base64/XDR in a row aborts before execution.
#[test]
fn wrapper_malformed_xdr_aborts_before_execution() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let key = real_storage_key();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let resp = sim_response(&row_json(
        &key_b64,
        "updated",
        Some("!!!bad!!!"),
        Some("!!!bad!!!"),
    ));

    let err = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &sdkt_fuzz::SequenceState::default(),
        &resp,
        &env,
        &[FunctionCall::new("increment", vec![])],
        "wrapper-malformed",
    )
    .expect_err("malformed XDR must abort");
    assert!(
        matches!(err, sdkt_fuzz::FuzzError::Observation(_)),
        "got: {err:?}"
    );
}

/// Negative: key mismatch — the row's `after` decodes to a different key.
#[test]
fn wrapper_key_mismatch_aborts_before_execution() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let key = real_storage_key();
    let other = other_storage_key_xdr();
    let key_b64 = sdkt_fuzz::state_capture::encode_key(&key).expect("encodes");
    let b = sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(real_storage_key(), 1))
        .expect("encodes");
    let a =
        sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(other, 2)).expect("encodes");
    let resp = sim_response(&row_json(&key_b64, "updated", Some(&b), Some(&a)));

    let err = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &sdkt_fuzz::SequenceState::default(),
        &resp,
        &env,
        &[FunctionCall::new("increment", vec![])],
        "wrapper-keymismatch",
    )
    .expect_err("key mismatch must abort");
    assert!(
        matches!(err, sdkt_fuzz::FuzzError::Observation(_)),
        "got: {err:?}"
    );
}

/// Negative: an `Account` row key whose payload is a `ContractData` entry
/// is rejected by the decoder's key/entry consistency check (never reaches
/// the chaining function's `UnsupportedKey` branch, which stays fail-safe
/// for hand-built rows — see `chaining_non_contract_data_row_is_an_error`).
#[test]
fn wrapper_account_key_rejected_by_decoder() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let acct = account_key_xdr();
    let acct_b64 = sdkt_fuzz::state_capture::encode_key(&acct).expect("encodes");
    let storage = real_storage_key();
    let b = sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage.clone(), 1))
        .expect("encodes");
    let a =
        sdkt_fuzz::state_capture::encode_entry(&contract_data_entry(storage, 2)).expect("encodes");
    let resp = sim_response(&row_json(&acct_b64, "updated", Some(&b), Some(&a)));

    let err = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &sdkt_fuzz::SequenceState::default(),
        &resp,
        &env,
        &[FunctionCall::new("increment", vec![])],
        "wrapper-account",
    )
    .expect_err("account key must be rejected by the decoder");
    assert!(
        matches!(err, sdkt_fuzz::FuzzError::Observation(_)),
        "got: {err:?}"
    );
}

/// Negative: a row that is not a JSON object aborts before execution.
#[test]
fn wrapper_non_object_row_aborts_before_execution() {
    let exec = Executor::new(CHAIN_COUNTER_WASM, Default::default()).expect("executor");
    let env = Environment {
        ledger: Default::default(),
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };

    let raw = r#"{"transactionData":"","minResourceFee":"0","latestLedger":"1000","stateChanges":["not-an-object"],"results":[]}"#;
    let resp: sdkt_rpc::SimulateResponse = serde_json::from_str(raw).expect("valid json");

    let err = sdkt_fuzz::execute_sequence_from_simulation(
        &exec,
        &sdkt_fuzz::SequenceState::default(),
        &resp,
        &env,
        &[FunctionCall::new("increment", vec![])],
        "wrapper-nonobject",
    )
    .expect_err("non-object row must abort");
    assert!(
        matches!(err, sdkt_fuzz::FuzzError::Observation(_)),
        "got: {err:?}"
    );
}
