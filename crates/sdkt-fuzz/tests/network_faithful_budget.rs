//! `BudgetPlan::NetworkFaithful` integration tests.
//!
//! Covers what the unit tests in `environment.rs` cannot: a real host
//! execution under the network-faithful budget, cross-process determinism of
//! that execution, and the artifact snapshot round-trip.
//!
//! The network-faithful budget is built from the network-faithful Protocol 29
//! cost model derived from stellar-core protocol configuration (vendored
//! 23-entry CPU + memory tables, limits 2_500_000 / 2_000_000). It is not a
//! live Mainnet validator configuration.

use sdkt_fuzz::{artifact::EnvironmentSnapshot, BudgetPlan, Environment, Executor, FunctionCall};

const COUNTER_WASM: &[u8] = include_bytes!("../../../crates/sdkt-cli/tests/fixtures/us_new.wasm");

fn network_faithful_env() -> Environment {
    Environment {
        ledger: Default::default(),
        budget: BudgetPlan::NetworkFaithful,
    }
}

/// Digest of one `increment()` execution: status + budget counters.
fn execution_digest(plan: &BudgetPlan) -> String {
    let exec = Executor::new(COUNTER_WASM, Default::default()).unwrap();
    let env = Environment {
        ledger: Default::default(),
        budget: plan.clone(),
    };
    let obs = exec
        .execute_with(
            &exec.case("nf", FunctionCall::new("increment", vec![]), vec![]),
            &env,
        )
        .unwrap();
    format!(
        "{:?}|cpu={}|mem={}|rcpu={}|rmem={}",
        obs.status,
        obs.budget.consumed_cpu,
        obs.budget.consumed_mem,
        obs.budget.remaining_cpu,
        obs.budget.remaining_mem
    )
}

#[test]
fn network_faithful_executes_the_counter_contract() {
    let digest = execution_digest(&BudgetPlan::NetworkFaithful);
    assert!(
        digest.starts_with("Returned("),
        "increment must execute successfully under NetworkFaithful: {digest}"
    );
    assert!(
        !digest.contains("rcpu=100000000"),
        "NetworkFaithful must not fall back to the host default CPU limit: {digest}"
    );
    // The remaining budget must be the network ceiling minus what the run
    // consumed — i.e. a real charge against the network-faithful table.
    let rcpu: u64 = digest
        .split("rcpu=")
        .nth(1)
        .and_then(|s| s.split('|').next())
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        rcpu < 2_500_000,
        "a successful run must have consumed network budget: {digest}"
    );
}

#[test]
fn default_budget_execution_is_unchanged() {
    let digest = execution_digest(&BudgetPlan::Default);
    // Host default ceiling is 100M; one increment run leaves just under it.
    assert!(
        digest.contains("rcpu=99") && !digest.contains("rcpu=0"),
        "Default must keep the host default ceiling scale: {digest}"
    );
}

#[test]
fn network_faithful_execution_is_deterministic() {
    assert_eq!(
        execution_digest(&BudgetPlan::NetworkFaithful),
        execution_digest(&BudgetPlan::NetworkFaithful),
        "same inputs must produce the same digest"
    );
}

#[test]
fn network_faithful_snapshot_round_trips() {
    let env = network_faithful_env();
    let snap = EnvironmentSnapshot::from(&env);
    let json = serde_json::to_string(&snap).unwrap();
    assert!(
        json.contains(r#""kind":"network_faithful""#),
        "snapshot must record the plan kind: {json}"
    );
    let back: EnvironmentSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(Environment::from(&back), env);
}

#[test]
fn cross_process_determinism() {
    if let Ok(expected) = std::env::var("SDKT_FUZZ_NF_CHILD") {
        print!("{}", execution_digest(&BudgetPlan::NetworkFaithful));
        let _ = expected;
        return;
    }
    let here = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(&here)
        .args([
            "--exact",
            "cross_process_determinism",
            "--nocapture",
            "--quiet",
        ])
        .env("SDKT_FUZZ_NF_CHILD", "1")
        .output()
        .expect("spawn self as child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let child = stdout
        .trim_end()
        .rsplit(|c: char| c.is_whitespace())
        .find(|tok| !tok.is_empty() && tok.contains('|'))
        .map(|t| t.trim_end_matches('.').to_string())
        .unwrap_or_default();
    assert!(
        !child.is_empty(),
        "child produced no digest: stdout={stdout:?}"
    );
    assert_eq!(
        execution_digest(&BudgetPlan::NetworkFaithful),
        child,
        "same inputs must produce the same digest across processes"
    );
}
