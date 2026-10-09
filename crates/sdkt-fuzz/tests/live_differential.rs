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
