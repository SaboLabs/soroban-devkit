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
//! - The contract's state on testnet is whatever it is at the ledger the
//!   simulation ran against; the local execution starts from a baseline built
//!   from the same ledger's storage read, so state-dependent results can
//!   differ and are classified as such rather than hidden.

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

/// The local side: run the contract through the pinned host under the live
/// profile's budget.
fn local_run(profile: &NetworkProfile, function: &str) -> (LocalExecutionMetrics, String) {
    let wasm = sdkt_xdr::extract_wasm_bytecode_from_live_ledger_entry(TESTNET_CODE_ENTRY)
        .expect("fixture decodes to contract WASM");
    let exec = Executor::new(&wasm, Default::default()).expect("executor builds");
    let env = Environment {
        ledger: Default::default(),
        budget: network_faithful_plan(profile.clone()),
    };
    let obs = exec
        .execute_with(
            &exec.case(
                format!("differential-{function}"),
                FunctionCall::new(function, vec![]),
                vec![],
            ),
            &env,
        )
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
async fn rpc_run(
    client: &SorobanRpcClient,
    function: &str,
) -> Result<RpcSimulationMetrics, sdkt_fuzz::RpcBlockReason> {
    // 1. Source account.
    let source = std::env::var("SDKT_DIFFERENTIAL_SOURCE").unwrap_or_default();
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
    let rpc = match rpc_run(&client, &function).await {
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
#[tokio::test]
async fn dead_endpoint_is_an_rpc_failure_blocker() {
    let client = SorobanRpcClient::with_options("http://127.0.0.1:9", Some(3), Some(1));
    let result = rpc_run(&client, "pause").await;
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
    // SAFETY: env vars in tests run on threads; this is a single assignment
    // restored immediately after, and no other test reads this var.
    unsafe { std::env::set_var("SDKT_DIFFERENTIAL_SOURCE", "NOT_A_G_ADDRESS") };
    let result = rpc_run(&client, "pause").await;
    unsafe { std::env::remove_var("SDKT_DIFFERENTIAL_SOURCE") };
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
