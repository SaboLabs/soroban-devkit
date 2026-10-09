//! Network-faithful profile and differential comparison integration tests.
//!
//! These cover the acceptance criteria the unit tests cannot: snapshot
//! round-trips through files, cross-network/cross-protocol rejection, the
//! `NetworkFaithful` plan's refusal behaviour, and the differential
//! classification ladder.

use sdkt_fuzz::{
    compare_differential, network_faithful_plan, profile_from_capture, BudgetCoverage, BudgetPlan,
    CaptureInput, DifferentialRecord, Environment, LocalExecutionMetrics, MismatchClass,
    NetworkProfile, ProfileStatus, RpcSimulationMetrics, HOST_SUPPORTED_PROTOCOL,
};

const MAINNET: &str = "Public Global Stellar Network ; September 2015";
const TESTNET: &str = "Test SDF Network ; September 2015";

/// A capture shaped like the live mainnet one (protocol 29, 86 cost types).
fn mainnet_capture(protocol: u32) -> CaptureInput {
    let names: Vec<String> = soroban_env_host::xdr::ContractCostType::variants()
        .iter()
        .map(|v| v.name().to_string())
        .collect();
    let params: Vec<sdkt_fuzz::CostParamEntrySnapshot> = (0..86u32)
        .map(|i| sdkt_fuzz::CostParamEntrySnapshot {
            cost_type: names[i as usize].clone(),
            index: i,
            const_term: 4 + i as i64,
            linear_term: 0,
        })
        .collect();
    CaptureInput {
        passphrase: MAINNET.to_string(),
        protocol_version: protocol,
        ledger_sequence: 64_846_678,
        source_endpoint: "https://mainnet.sorobanrpc.com".to_string(),
        captured_at_unix: 1_791_518_867,
        cpu_limit: Some((400_000_000, 62_447_231)),
        mem_limit: Some((41_943_040, 62_447_231)),
        ledger_max_instructions: Some((580_000_000, 62_447_231)),
        fee_rate_per_instructions_increment: Some((7, 62_447_231)),
        max_contract_size_bytes: Some((131_072, 60_993_066)),
        tx_max_size_bytes: Some((132_096, 60_993_066)),
        tx_max_contract_events_size_bytes: Some((16_384, 60_993_066)),
        fee_contract_events_1kb: Some((200, 60_993_066)),
        cpu_cost_params: Some(params.clone()),
        mem_cost_params: Some(params),
        cost_params_observed_at: Some(62_447_231),
    }
}

fn write_and_load(profile: &NetworkProfile) -> NetworkProfile {
    let dir = std::env::temp_dir().join(format!("sdkt-profile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profile.json");
    std::fs::write(&path, serde_json::to_string_pretty(profile).unwrap()).unwrap();
    let back: NetworkProfile =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    back
}

// --- 1. Snapshot validation and reproducibility ---------------------------

#[test]
fn snapshot_round_trips_through_a_file_with_identical_hash() {
    let p = profile_from_capture(mainnet_capture(29));
    let loaded = write_and_load(&p);
    assert_eq!(loaded.content_hash(), p.content_hash());
    assert_eq!(loaded.content_hash_hex(), p.content_hash_hex());
    assert_eq!(loaded.status(), p.status());
    // The pinned host is protocol 29, so a fully-observed protocol-29
    // mainnet profile is complete and its budget builds.
    assert_eq!(loaded.status(), ProfileStatus::Complete);
    loaded.validate(None).unwrap();
}

#[test]
fn tampered_snapshot_is_rejected_by_hash_and_validation() {
    let p = profile_from_capture(mainnet_capture(29));
    let mut tampered = p.clone();
    tampered.ledger_sequence = 64_846_679;
    // The hash changes, so a tampered snapshot is detectable.
    assert_ne!(tampered.content_hash(), p.content_hash());
    // And a tampered network id fails validation outright.
    let mut bad = p.clone();
    bad.network_id = [7u8; 32];
    assert!(bad.validate(None).is_err());
}

#[test]
fn stale_snapshot_against_a_current_ledger_is_rejected() {
    let p = profile_from_capture(mainnet_capture(29));
    let current = p.ledger_sequence + 10_000;
    assert!(p.validate(Some(current)).is_err());
}

// --- 2. Network / protocol mismatch rejection -----------------------------

#[test]
fn mainnet_and_testnet_profiles_never_compare_equal() {
    let mut testnet = mainnet_capture(29);
    testnet.passphrase = TESTNET.to_string();
    let m = profile_from_capture(mainnet_capture(29));
    let t = profile_from_capture(testnet);
    assert_ne!(m.content_hash(), t.content_hash());
    assert_ne!(m.network_id, t.network_id);
}

#[test]
fn a_protocol_above_the_host_is_refused_by_budget_construction() {
    // The guard still exists: a profile one protocol above the pinned host
    // must be refused, not silently executed.
    let p = profile_from_capture(mainnet_capture(HOST_SUPPORTED_PROTOCOL + 1));
    let plan = network_faithful_plan(p);
    let env = Environment {
        ledger: Default::default(),
        budget: plan,
    };
    let err = env.make_budget().unwrap_err().to_string();
    assert!(
        err.contains("HOST_UNSUPPORTED"),
        "refusal must name the coverage verdict: {err}"
    );
}

#[test]
fn a_mainnet_protocol_29_profile_now_builds_a_budget() {
    // The whole point of the host upgrade: the same live mainnet profile that
    // was refused under host 28 now produces a budget.
    let p = profile_from_capture(mainnet_capture(29));
    assert_eq!(p.status(), ProfileStatus::Complete);
    let env = Environment {
        ledger: Default::default(),
        budget: network_faithful_plan(p),
    };
    let budget = env.make_budget().unwrap();
    assert_eq!(budget.get_cpu_insns_remaining().unwrap(), 400_000_000);
    assert_eq!(budget.get_mem_bytes_remaining().unwrap(), 41_943_040);
}

// --- 3. Missing / stale data is not treated as a valid Mainnet config -----

#[test]
fn missing_parameters_are_incomplete_not_zero() {
    let mut c = mainnet_capture(28);
    c.cpu_limit = None;
    c.mem_limit = None;
    c.cpu_cost_params = None;
    c.mem_cost_params = None;
    c.cost_params_observed_at = None;
    let p = profile_from_capture(c);
    assert_eq!(p.status(), ProfileStatus::Incomplete);
    let plan = network_faithful_plan(p);
    let env = Environment {
        ledger: Default::default(),
        budget: plan,
    };
    let err = env.make_budget().unwrap_err().to_string();
    assert!(
        err.contains("INCOMPLETE"),
        "refusal must name the verdict: {err}"
    );
}

#[test]
fn a_protocol_29_profile_with_full_config_is_complete() {
    // The pinned host is protocol 29: a fully-observed protocol-29 profile
    // is complete and its budget builds with the live limits.
    let p = profile_from_capture(mainnet_capture(29));
    assert_eq!(p.status(), ProfileStatus::Complete);
    assert_eq!(HOST_SUPPORTED_PROTOCOL, 29);
    let coverage = BudgetCoverage::for_profile(&p, sdkt_fuzz::environment::host_cost_type_count());
    assert!(coverage.complete);
    assert_eq!(coverage.as_str(), "COMPLETE");
    // And the budget actually builds with the live limits.
    let env = Environment {
        ledger: Default::default(),
        budget: network_faithful_plan(p),
    };
    let budget = env.make_budget().unwrap();
    assert_eq!(budget.get_cpu_insns_remaining().unwrap(), 400_000_000);
    assert_eq!(budget.get_mem_bytes_remaining().unwrap(), 41_943_040);
}

// --- 4. No cost type is silently free ------------------------------------

#[test]
fn coverage_reports_the_host_cost_type_count() {
    let p = profile_from_capture(mainnet_capture(28));
    let coverage = BudgetCoverage::for_profile(&p, sdkt_fuzz::environment::host_cost_type_count());
    // The live table covers exactly as many types as the host knows, so the
    // model is complete for this host.
    assert_eq!(coverage.covered_cost_types, 86);
    assert_eq!(coverage.host_cost_types, 86);
    assert!(coverage.complete);
}

#[test]
fn a_short_table_is_reported_incomplete_not_silently_free() {
    let mut c = mainnet_capture(28);
    // Truncate to the 23-entry protocol-initial table.
    let cpu = c.cpu_cost_params.take().unwrap();
    let mem = c.mem_cost_params.take().unwrap();
    c.cpu_cost_params = Some(cpu[..23].to_vec());
    c.mem_cost_params = Some(mem[..23].to_vec());
    let p = profile_from_capture(c);
    let coverage = BudgetCoverage::for_profile(&p, sdkt_fuzz::environment::host_cost_type_count());
    assert!(!coverage.complete);
    assert_eq!(coverage.as_str(), "INCOMPLETE");
    assert_eq!(coverage.covered_cost_types, 23);
    assert_eq!(coverage.host_cost_types, 86);
}

// --- 5. Differential classification --------------------------------------

fn local(protocol: u32, cpu: u64, mem: u64, ok: bool) -> LocalExecutionMetrics {
    LocalExecutionMetrics {
        ledger_sequence: 64_846_678,
        protocol_version: protocol,
        cpu_insns: cpu,
        mem_bytes: mem,
        succeeded: ok,
        error_type: None,
    }
}

fn rpc(protocol: u32, cpu: u64, mem: u64, error: bool) -> RpcSimulationMetrics {
    RpcSimulationMetrics {
        ledger_sequence: 64_846_678,
        protocol_version: protocol,
        cpu_insns: cpu,
        mem_bytes: mem,
        error,
    }
}

#[test]
fn protocol_mismatch_classifies_before_everything_else() {
    let p = profile_from_capture(mainnet_capture(29));
    let rec = compare_differential(
        "c1",
        "increment",
        &p,
        local(28, 100, 100, true),
        rpc(29, 100, 100, false),
        0.01,
        0.01,
    );
    assert_eq!(rec.classification, MismatchClass::ProtocolMismatch);
    assert!(rec.reason.contains("protocol"));
}

#[test]
fn matching_execution_classifies_as_match() {
    let p = profile_from_capture(mainnet_capture(28));
    let rec = compare_differential(
        "c2",
        "increment",
        &p,
        local(28, 100_000, 50_000, true),
        rpc(28, 100_000, 50_000, false),
        0.01,
        0.01,
    );
    assert_eq!(rec.classification, MismatchClass::Match);
}

#[test]
fn resource_drift_classifies_with_the_tolerances_recorded() {
    let p = profile_from_capture(mainnet_capture(28));
    let rec = compare_differential(
        "c3",
        "increment",
        &p,
        local(28, 500_000, 50_000, true),
        rpc(28, 100_000, 50_000, false),
        0.01,
        0.01,
    );
    assert_eq!(rec.classification, MismatchClass::ResourceDrift);
    assert_eq!(rec.cpu_tolerance(), 0.01);
    assert_eq!(rec.mem_tolerance(), 0.01);
    assert!(rec.reason.contains("resource drift"));
}

#[test]
fn execution_divergence_classifies_separately() {
    let p = profile_from_capture(mainnet_capture(28));
    let rec = compare_differential(
        "c4",
        "increment",
        &p,
        local(28, 100_000, 50_000, true),
        rpc(28, 100_000, 50_000, true),
        0.01,
        0.01,
    );
    assert_eq!(rec.classification, MismatchClass::ExecutionDivergence);
}

#[test]
fn differential_record_round_trips_with_reproduction_metadata() {
    let p = profile_from_capture(mainnet_capture(28));
    let rec = compare_differential(
        "c5",
        "increment",
        &p,
        local(28, 100_000, 50_000, true),
        rpc(28, 100_000, 50_000, false),
        0.02,
        0.02,
    );
    let json = serde_json::to_string(&rec).unwrap();
    let back: DifferentialRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, rec);
    assert_eq!(back.profile_content_hash, p.content_hash_hex());
    assert_eq!(back.cpu_tolerance(), 0.02);
}

// --- 6. Old plans keep working -------------------------------------------

#[test]
fn default_and_protocol_initial_plans_are_unchanged() {
    let default = Environment::default().make_budget().unwrap();
    assert_eq!(default.get_cpu_insns_remaining().unwrap(), 100_000_000);

    let initial = Environment {
        ledger: Default::default(),
        budget: BudgetPlan::ProtocolInitial,
    }
    .make_budget()
    .unwrap();
    assert_eq!(initial.get_cpu_insns_remaining().unwrap(), 2_500_000);
    assert_eq!(initial.get_mem_bytes_remaining().unwrap(), 2_000_000);

    let capped = Environment {
        ledger: Default::default(),
        budget: BudgetPlan::Capped { cpu: 7, mem: 9 },
    }
    .make_budget()
    .unwrap();
    assert_eq!(capped.get_cpu_insns_remaining().unwrap(), 7);
    assert_eq!(capped.get_mem_bytes_remaining().unwrap(), 9);
}

#[test]
fn snapshot_kind_records_the_profile_hash_and_coverage() {
    let p = profile_from_capture(mainnet_capture(29));
    let env = Environment {
        ledger: Default::default(),
        budget: network_faithful_plan(p.clone()),
    };
    let snap = sdkt_fuzz::artifact::EnvironmentSnapshot::from(&env);
    let json = serde_json::to_string(&snap).unwrap();
    assert!(json.contains(r#""kind":"network_faithful""#), "{json}");
    assert!(json.contains(&p.content_hash_hex()));
    assert!(json.contains("COMPLETE"));
}

#[test]
fn observed_values_carry_provenance() {
    let p = profile_from_capture(mainnet_capture(29));
    let cpu = &p.config.cpu_limit;
    assert_eq!(cpu.value, Some(400_000_000));
    assert_eq!(cpu.provenance.observed_at_ledger, Some(62_447_231));
    assert_eq!(
        cpu.provenance.source,
        sdkt_fuzz::ParamSource::LiveConfigSetting
    );
    // A never-captured parameter is Unavailable, not zero.
    let mut c = mainnet_capture(29);
    c.fee_contract_events_1kb = None;
    let p2 = profile_from_capture(c);
    assert_eq!(p2.config.fee_contract_events_1kb.value, None);
    assert_eq!(
        p2.config.fee_contract_events_1kb.provenance.source,
        sdkt_fuzz::ParamSource::Unavailable
    );
}
