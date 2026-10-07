//! Phase 3 test D: artifact hash + replay result across processes.
//!
//! Re-executes this test binary as a child with a marker env var; parent
//! and child must produce identical artifact canonical hashes and identical
//! replay verdict hashes for the same campaign.

use std::collections::BTreeMap;

use sdkt_fuzz::{run_campaign_artifacts, CampaignInput, Environment, Expected, ExpectedBehavior};
use soroban_env_host::xdr::ScVal;

const COUNTER_WASM: &[u8] = include_bytes!("../../../crates/sdkt-cli/tests/fixtures/us_new.wasm");

/// The whole pipeline digest: campaign artifacts + one replay verdict each.
fn digest() -> String {
    let mut input = CampaignInput::new(COUNTER_WASM, [123u8; 32]).unwrap();
    input.cases = 4;
    input.mutations_per_case = 2;
    input.environment = Environment::default();

    let mut expectations = BTreeMap::new();
    expectations.insert(
        "hello".to_string(),
        Expected {
            behavior: ExpectedBehavior::Success {
                expect_return: Some(ScVal::U32(43)),
            },
            ..Default::default()
        },
    );

    let (_, artifacts) = run_campaign_artifacts(&input, &expectations).unwrap();
    let mut out = String::new();
    for a in &artifacts {
        out.push_str(&a.canonical_hash());
        let outcome = sdkt_fuzz::replay(a, COUNTER_WASM).expect("replay runs");
        out.push('|');
        out.push_str(&outcome.canonical_hash());
        out.push('|');
    }
    out
}

#[test]
fn artifact_and_replay_are_cross_process_deterministic() {
    if std::env::var_os("SDKT_FUZZ_CHILD").is_some() {
        print!("{}", digest());
        return;
    }

    let here = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(&here)
        .args([
            "--exact",
            "artifact_and_replay_are_cross_process_deterministic",
            "--nocapture",
            "--quiet",
        ])
        .env("SDKT_FUZZ_CHILD", "1")
        .output()
        .expect("spawn self as child");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mine = digest();
    assert!(
        stdout.contains(&mine),
        "child digest must equal the parent's.\nparent: {mine}\nchild stdout: {stdout:?}\nchild stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !mine.is_empty(),
        "campaign must produce artifacts to compare"
    );
}
