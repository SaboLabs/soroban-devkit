//! Phase 2 cross-process determinism.
//!
//! The same campaign digest must be produced by a *separate OS process*.
//! The test re-executes its own test binary as a child with an env marker
//! and compares the digests.

use std::process::Command;

use sdkt_fuzz::{Environment, Executor, FunctionCall, GenerationCaps};
use soroban_env_host::xdr::ScVal;

const COUNTER_WASM: &[u8] = include_bytes!("../../../crates/sdkt-cli/tests/fixtures/us_new.wasm");
const AUTH_WASM: &[u8] = include_bytes!("fixtures/auth_probe.wasm");

/// Digest of a small battery of deterministic operations. Pure function of
/// the inputs — no timestamps, no addresses, no debug strings.
fn digest() -> String {
    let mut out = String::new();

    // Generation battery.
    let spec = sdkt_wasm::parse_contract_spec(AUTH_WASM).unwrap();
    for (seed, case) in [([1u8; 32], "a"), ([2u8; 32], "b")] {
        let g = sdkt_fuzz::generator::generate_call(
            &spec,
            "set",
            &seed,
            case,
            GenerationCaps::default(),
        )
        .unwrap();
        out.push_str(&format!("{:?}|", g.args));
    }

    // Execution battery.
    let exec = Executor::new(COUNTER_WASM, Default::default()).unwrap();
    let env = Environment::default();
    let run = sdkt_fuzz::execute_sequence(
        &exec,
        &[],
        &env,
        &[
            FunctionCall::new("increment", vec![]),
            FunctionCall::new("increment", vec![]),
        ],
        "x",
    )
    .unwrap();
    out.push_str(&format!("{:?}", run.final_observation.status));
    for s in &run.steps {
        out.push_str(&format!("{:?}", s.observation.return_value()));
    }
    out
}

#[test]
fn cross_process_determinism() {
    if let Ok(expected) = std::env::var("SDKT_FUZZ_CHILD_DIGEST") {
        // Child mode: print the digest and exit.
        print!("{}", digest());
        let _ = expected;
        return;
    }

    let here = std::env::current_exe().expect("test binary path");
    let out = Command::new(&here)
        .args([
            "--exact",
            "cross_process_determinism",
            "--nocapture",
            "--quiet",
        ])
        .env("SDKT_FUZZ_CHILD_DIGEST", "1")
        .output()
        .expect("spawn self as child");

    let stdout = String::from_utf8_lossy(&out.stdout);
    // The harness appends a trailing period after the child's print; the
    // digest itself contains no period.
    let child_digest = stdout
        .trim_end()
        .rsplit(|c: char| c.is_whitespace())
        .find(|tok| !tok.is_empty() && tok.contains('|'))
        .map(|t| t.trim_end_matches('.').to_string())
        .unwrap_or_default();

    let mine = digest();
    assert!(
        !child_digest.is_empty(),
        "child produced no digest: stdout={stdout:?} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        mine, child_digest,
        "same inputs must produce the same digest across processes"
    );
}
