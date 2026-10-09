//! Live state-matched differential integration test.
//!
//! One end-to-end run against public Testnet: simulate a read-only contract
//! invocation, capture the ledger state the simulation used, verify the
//! capture against the consistency guards, execute the same call locally
//! under the captured state, and compare **outcomes** — never CPU/memory
//! metrics (see the module-level note at the bottom).
//!
//! ## Running it
//!
//! ```text
//! cargo test -p sdkt-fuzz --test state_matched_live -- --ignored --nocapture
//! ```
//!
//! It is `#[ignore]`d by default: normal CI runs the deterministic suite
//! only, and this test needs the public Testnet RPC to be reachable.
//! Nothing here submits a transaction, signs anything, or needs a secret —
//! the source account is an ephemeral key generated in-process, and
//! `simulateTransaction` / `getLedgerEntries` are both read-only calls.
//!
//! ## What a pass proves, and what it does not
//!
//! A pass proves the *pipeline*: footprint extraction from a live
//! `simulateTransaction`, ledger-state fetch, consistency verification
//! (fetch freshness, entry liveness, entry stability), a stateChanges
//! cross-check, and a local execution that resolves its contract code and
//! instance from the network's own entries. It also proves that, for **this
//! one read-only function at this ledger**, the RPC and the local host agree
//! on the outcome (both returned a result).
//!
//! A pass does **not** prove:
//! - contract-wide or function-wide parity (one read-only function is not a
//!   survey);
//! - CPU/memory parity — the RPC no longer reports a `cost` block, so there
//!   are no comparable numbers on the RPC side at all
//!   ([`sdkt_fuzz::RpcBlockReason::MissingCost`] is the standing blocker);
//! - that the network's *state* was reproduced bit-for-bit — the guards
//!   verify consistency conditions that are necessary, not sufficient (see
//!   the module doc on `sdkt_fuzz::state_capture`).
//!
//! A fail is reported as the pipeline's own structured blocker (with its
//! category) wherever one exists, rather than a bare assertion failure, so
//! a network-state change is distinguishable from a code regression.

use sdkt_fuzz::state_capture::{
    build_capture, footprint_keys_from_transaction_data, verify_against_state_changes, RawCapture,
};
use sdkt_rpc::{fetch_ledger_entries, SorobanRpcClient};
use stellar_strkey::Strkey;

/// The public Testnet contract the repo's walkthrough uses, and the read-only
/// function the corpus already exercises. Both are proven inputs: if either
/// changes on the network, the diagnostics below will say which guard
/// caught it.
const TESTNET_CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
const TARGET_FUNCTION: &str = "get_all_order_ids";
const TESTNET_RPC: &str = "https://soroban-testnet.stellar.org";
const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

/// An in-process ephemeral keypair: the source account for the envelope.
/// Simulation checks that the envelope is well-formed, not that the account
/// exists or is funded, and nothing is submitted — so no real identity is
/// needed and none is configured.
fn ephemeral_source() -> String {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("OS randomness");
    Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(
        sdkt_xdr::Ed25519Signer::from_seed(&seed).public_key_bytes_owned(),
    ))
    .to_string()
    .to_string()
}

#[tokio::test]
#[ignore = "live: requires the public Testnet RPC; run with --ignored"]
async fn live_state_matched_read_only_outcome_agrees() {
    let client = SorobanRpcClient::new(TESTNET_RPC);

    // --- Pre-flight: the endpoint must be the network the test claims. ---
    let network = client
        .get_network()
        .await
        .expect("Testnet RPC reachable (getNetwork)");
    assert_eq!(
        network.protocol_version, 29,
        "Testnet must report protocol 29 (the pinned host's protocol)"
    );
    assert_eq!(
        network.passphrase, TESTNET_PASSPHRASE,
        "endpoint must be Testnet, not another network"
    );
    println!(
        "PRE-FLIGHT: protocol={} passphrase ok",
        network.protocol_version
    );

    // --- 1. Simulate the read-only call. ---
    let params = sdkt_xdr::InvokeTransactionParams {
        source_account: ephemeral_source(),
        sequence: 1,
        fee: 100,
        contract_id: TESTNET_CONTRACT.to_string(),
        function: TARGET_FUNCTION.to_string(),
        args: vec![],
        memo: None,
    };
    let envelope = sdkt_xdr::build_invoke_transaction(&params).expect("envelope builds");
    let response = sdkt_rpc::simulate_transaction(&client, &envelope)
        .await
        .expect("simulateTransaction reachable");
    assert!(
        response.error.is_none(),
        "the read-only call must simulate cleanly; got error: {:?}",
        response.error
    );
    // CPU/memory parity is out of scope by protocol: the RPC does not report
    // a cost block. Confirm the standing blocker rather than papering over it.
    assert!(
        response.cost.is_none(),
        "unexpected cost block: if the network started returning one, this \
         test's scope (outcome-only comparison) should be revisited — but do \
         NOT map cost fields onto local CPU/memory here"
    );
    let sim_ledger: u32 = response
        .latest_ledger
        .as_deref()
        .expect("latestLedger")
        .parse()
        .expect("ledger number");
    println!(
        "SIMULATE: ledger={sim_ledger} txData={}B minResourceFee={} results={}",
        response.transaction_data.len(),
        response.min_resource_fee,
        response.results.len()
    );

    // --- 2. Extract the footprint the simulation declared. ---
    let footprint = footprint_keys_from_transaction_data(&response.transaction_data)
        .expect("footprint decodes");
    println!("FOOTPRINT: {} keys", footprint.len());

    // --- 3. Fetch the current ledger state for every footprint key. ---
    let storage = fetch_ledger_entries(&client, &footprint)
        .await
        .expect("getLedgerEntries reachable");
    let fetch_ledger = storage.latest_ledger;
    println!(
        "FETCH: ledger={fetch_ledger} entries={} (sim={sim_ledger})",
        storage.entries.len()
    );

    // --- 4. Build the capture and run the consistency guards. ---
    let mut raw: Vec<RawCapture> = Vec::new();
    let mut fetched_keys: Vec<String> = Vec::new();
    for e in &storage.entries {
        // `getLedgerEntries.xdr` is the LedgerEntryData union; the sequence
        // and ext arrive separately as envelope fields.
        let data = sdkt_rpc::network_capture::decode_ledger_entry_data_b64(&e.xdr)
            .expect("entry data decodes");
        raw.push(RawCapture {
            key_b64: e.key.clone(),
            entry: Some(stellar_xdr::LedgerEntry {
                last_modified_ledger_seq: e.last_modified_ledger_seq,
                data,
                ext: stellar_xdr::LedgerEntryExt::V0,
            }),
            last_modified_ledger_seq: e.last_modified_ledger_seq,
            live_until_ledger_seq: e.live_until_ledger_seq,
        });
        fetched_keys.push(e.key.clone());
    }
    for k in &footprint {
        if !fetched_keys.contains(k) {
            raw.push(RawCapture {
                key_b64: k.clone(),
                entry: None,
                last_modified_ledger_seq: 0,
                live_until_ledger_seq: None,
            });
        }
    }

    let outcome = build_capture(&footprint, &raw, fetch_ledger, sim_ledger);
    if let Some(reason) = outcome.block_reason() {
        panic!(
            "state capture BLOCKED [{}]: {reason:?} — the ledger state cannot \
             be verified as the state the simulation used, so no \
             state-matched comparison is possible this run",
            reason.category()
        );
    }
    let capture_ledger = outcome.fetch_latest_ledger();
    println!("CAPTURE: VERIFIED (fetch={capture_ledger} sim={sim_ledger})");

    // --- 5. Cross-check the capture against the simulation's own before-values.
    let state_changes: Vec<(String, String)> = response
        .state_changes
        .iter()
        .filter_map(|v| {
            let key = v.get("key")?.as_str()?.to_string();
            let before = v.get("before")?.as_str()?.to_string();
            Some((key, before))
        })
        .collect();
    println!(
        "STATE_CHANGES with before values: {} (a read-only call usually has none)",
        state_changes.len()
    );
    let checked = verify_against_state_changes(&outcome, &state_changes);
    if let Some(reason) = checked.block_reason() {
        panic!(
            "stateChanges cross-check BLOCKED [{}]: {reason:?} — the captured \
             state disagrees with the state the simulation reports using",
            reason.category()
        );
    }
    let entries = checked.verified_entries().expect("verified capture");
    println!("CROSS-CHECK: verified ({} entries)", entries.len());

    // --- 6. Execute locally under the captured state. ---
    let code_entry = entries
        .iter()
        .find(|e| matches!(&e.entry.data, stellar_xdr::LedgerEntryData::ContractCode(_)))
        .expect("ContractCode must be in the footprint of any invocation");
    let wasm = if let stellar_xdr::LedgerEntryData::ContractCode(code) = &code_entry.entry.data {
        println!("LOCAL: network ContractCode ({} bytes)", code.code.len());
        code.code.to_vec()
    } else {
        unreachable!("found entry is ContractCode")
    };
    let exec = sdkt_fuzz::Executor::new(&wasm, Default::default()).expect("executor");

    // The executor derives the case's contract address deterministically;
    // re-point the captured instance + data entries at that address so the
    // host resolves the contract to the network's own code and storage.
    let scratch = exec.case(
        "scratch",
        sdkt_fuzz::FunctionCall::new(TARGET_FUNCTION, vec![]),
        vec![],
    );
    let contract = scratch.contract_address();
    let instance_entry = entries
        .iter()
        .find(|e| {
            matches!(
                &e.entry.data,
                stellar_xdr::LedgerEntryData::ContractData(cd) if matches!(cd.key, stellar_xdr::ScVal::LedgerKeyContractInstance)
            )
        })
        .map(|e| {
            let mut e = e.entry.clone();
            if let stellar_xdr::LedgerEntryData::ContractData(cd) = &mut e.data {
                cd.contract = contract.clone();
            }
            e
        })
        .expect("instance entry must be in the footprint");
    let data_entries: Vec<stellar_xdr::LedgerEntry> = entries
        .iter()
        .filter(|e| {
            matches!(
                &e.entry.data,
                stellar_xdr::LedgerEntryData::ContractData(cd) if !matches!(cd.key, stellar_xdr::ScVal::LedgerKeyContractInstance)
            )
        })
        .map(|e| {
            let mut e = e.entry.clone();
            if let stellar_xdr::LedgerEntryData::ContractData(cd) = &mut e.data {
                cd.contract = contract.clone();
            }
            e
        })
        .collect();
    println!(
        "LOCAL: baseline = external instance + {} data entries, ledger={sim_ledger}",
        data_entries.len()
    );

    let case = exec
        .case(
            format!("state-matched-{TARGET_FUNCTION}"),
            sdkt_fuzz::FunctionCall::new(TARGET_FUNCTION, vec![]),
            data_entries,
        )
        .with_external_instance_entry(instance_entry);
    let env = sdkt_fuzz::Environment {
        ledger: sdkt_fuzz::LedgerConfig {
            sequence_number: sim_ledger,
            timestamp: 0,
        },
        budget: sdkt_fuzz::BudgetPlan::ProtocolInitial,
    };
    let obs = exec
        .execute_with(&case, &env)
        .expect("local execution under the captured state");
    println!(
        "LOCAL: status={:?} cpu={} mem={} (cpu/mem are local-only numbers, \
         not compared to anything)",
        obs.status, obs.budget.consumed_cpu, obs.budget.consumed_mem
    );

    // --- 7. Compare outcomes (never CPU/memory). ---
    let rpc_returned = !response.results.is_empty();
    let local_returned = obs.is_success();
    println!("OUTCOME: rpc_returned={rpc_returned} local_returned={local_returned}");
    assert_eq!(
        rpc_returned, local_returned,
        "outcome divergence for {TARGET_FUNCTION}: the RPC and the local host \
         disagree on whether the call produces a result. This is a real \
         finding to investigate, not a flake to silence."
    );
}
