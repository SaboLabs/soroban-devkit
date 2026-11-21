//! Run a deterministic, in-memory Soroban fuzz campaign against a committed fixture.
//! Run with: cargo run -p sdkt-fuzz --example small_campaign

use std::collections::BTreeMap;

use sdkt_fuzz::{run_campaign, CampaignInput, Expected};

const WASM: &[u8] = include_bytes!("../../sdkt-cli/tests/fixtures/us_new.wasm");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = CampaignInput::new(WASM, [21; 32])?;
    input.function = Some("hello".to_string());
    input.cases = 4;
    input.mutations_per_case = 0;
    input.sequence_every = 0;

    // No declared violation: a finding requires an explicit expectation.
    let expectations = BTreeMap::<String, Expected>::new();
    let result = run_campaign(&input, &expectations)?;
    if result.executed == 0 {
        return Err("fuzz campaign executed no cases".into());
    }

    println!("Executed: {}", result.executed);
    println!("Passed: {}", result.passed.len());
    println!("Expected errors: {}", result.expected_errors.len());
    println!("Findings: {}", result.findings.len());
    for finding in &result.findings {
        println!(
            "  {}: {} ({})",
            finding.identity.case_id,
            finding.identity.function,
            finding.reason_code.name()
        );
    }
    for skipped in &result.skipped_functions {
        println!("Skipped {}: {}", skipped.function, skipped.reason);
    }

    Ok(())
}
