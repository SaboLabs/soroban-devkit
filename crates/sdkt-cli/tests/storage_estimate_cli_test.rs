use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").expect("sdkt binary built")
}

const WASM_OLD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_old.wasm");
const WASM_NEW: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/us_new.wasm");

#[test]
fn storage_estimate_help_displays_flags_and_docs() {
    sdkt()
        .args(["storage", "estimate", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Estimate storage rent cost for a WASM contract offline",
        ))
        .stdout(predicate::str::contains("--ledgers"))
        .stdout(predicate::str::contains("--format"));
}

#[test]
fn storage_estimate_pretty_output_us_old() {
    sdkt()
        .args(["storage", "estimate", WASM_OLD])
        .assert()
        .success()
        .stdout(predicate::str::contains("Storage Cost Estimate for"))
        .stdout(predicate::str::contains(
            "Ledger Horizon: 17280 ledgers (~1 day(s) at 5s/ledger)",
        ))
        .stdout(predicate::str::contains(
            "Contract Spec:  2 function(s), 1 custom type(s), 1 event(s)",
        ))
        .stdout(predicate::str::contains("Instance:"))
        .stdout(predicate::str::contains("Entries:     1"))
        .stdout(predicate::str::contains(
            "Cost:        1728000 stroops (0.1728 XLM)",
        ))
        .stdout(predicate::str::contains("guaranteed_instance_singleton"))
        .stdout(predicate::str::contains("Persistent:"))
        .stdout(predicate::str::contains("runtime_state_unknown"))
        .stdout(predicate::str::contains("Temporary:"))
        .stdout(predicate::str::contains("Entries:     0"))
        .stdout(predicate::str::contains("Cost:        0 stroops (0 XLM)"))
        .stdout(predicate::str::contains("Total Estimate:"))
        .stdout(predicate::str::contains("Baseline Entries: 1"))
        .stdout(predicate::str::contains(
            "Total Cost:       1728000 stroops (0.1728 XLM)",
        ))
        .stdout(predicate::str::contains(
            "Approximation Ceiling & Limitations:",
        ));
}

#[test]
fn storage_estimate_pretty_output_us_new() {
    sdkt()
        .args(["storage", "estimate", WASM_NEW])
        .assert()
        .success()
        .stdout(predicate::str::contains("Storage Cost Estimate for"))
        .stdout(predicate::str::contains(
            "Contract Spec:  2 function(s), 0 custom type(s), 0 event(s)",
        ))
        .stdout(predicate::str::contains("Instance:"))
        .stdout(predicate::str::contains("Entries:     1"))
        .stdout(predicate::str::contains("Persistent:"))
        .stdout(predicate::str::contains("Entries:     0"))
        .stdout(predicate::str::contains("Cost:        0 stroops (0 XLM)"))
        .stdout(predicate::str::contains("runtime_state_unknown"))
        .stdout(predicate::str::contains("Total Estimate:"))
        .stdout(predicate::str::contains("Baseline Entries: 1"))
        .stdout(predicate::str::contains(
            "Total Cost:       1728000 stroops (0.1728 XLM)",
        ));
}

#[test]
fn storage_estimate_custom_ledgers() {
    sdkt()
        .args(["storage", "estimate", WASM_OLD, "--ledgers", "100"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Ledger Horizon: 100 ledgers"))
        .stdout(predicate::str::contains(
            "Cost:        10000 stroops (0.001 XLM)",
        ))
        .stdout(predicate::str::contains("Baseline Entries: 1"))
        .stdout(predicate::str::contains(
            "Total Cost:       10000 stroops (0.001 XLM)",
        ));
}

#[test]
fn storage_estimate_rejects_non_soroban_wasm() {
    let tmp = TempDir::new().unwrap();
    let plain = tmp.path().join("plain.wasm");
    fs::write(&plain, b"\x00asm\x01\x00\x00\x00").unwrap();

    sdkt()
        .args(["storage", "estimate", plain.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No contractspecv0 section found"));
}

#[test]
fn storage_estimate_rejects_missing_file() {
    sdkt()
        .args(["storage", "estimate", "/nonexistent/path/to/contract.wasm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read WASM"));
}
