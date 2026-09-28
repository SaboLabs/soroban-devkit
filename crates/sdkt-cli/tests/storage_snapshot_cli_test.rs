use assert_cmd::Command;
use predicates::prelude::*;
use sdkt_storage::{SnapshotDiff, SnapshotEntry, StorageClass, StorageSnapshot};
use tempfile::tempdir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

const TEST_CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

fn entry(key: &str, value: Option<&str>, ttl: u32, rent: u64) -> SnapshotEntry {
    SnapshotEntry {
        key: key.to_string(),
        class: StorageClass::Persistent,
        durability: Some("persistent".to_string()),
        current_ttl: ttl,
        extension_cost_stroops: rent,
        value: value.map(str::to_string),
    }
}

fn snapshot(contract_id: &str, entries: Vec<SnapshotEntry>) -> StorageSnapshot {
    StorageSnapshot {
        contract_id: contract_id.to_string(),
        captured_at_ledger: Some(1000),
        entries,
    }
}

#[test]
fn storage_diff_help_documents_against_flag() {
    sdkt()
        .args(["storage", "diff", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--against"))
        .stdout(predicate::str::contains("offline"));
}

#[test]
fn storage_diff_against_offline_two_snapshots_differing_in_each_direction() {
    let dir = tempdir().unwrap();
    let base_file = dir.path().join("base.json");
    let against_file = dir.path().join("against.json");

    // Entries:
    // - "k_unchanged": identical value and TTL on both sides (unchanged: 1)
    // - "k_removed": present in base, missing in against (removed: 1)
    // - "k_changed": present in both, with different value and different TTL (changed: value + ttl)
    // - "k_added": missing in base, present in against (added: 1)
    let base = snapshot(
        TEST_CONTRACT,
        vec![
            entry("k_unchanged", Some("AAAA"), 20000, 2_000_000),
            entry("k_removed", Some("BBBB"), 20000, 2_000_000),
            entry("k_changed", Some("CCCC"), 20000, 2_000_000),
        ],
    );

    let against = snapshot(
        TEST_CONTRACT,
        vec![
            entry("k_unchanged", Some("AAAA"), 20000, 2_000_000),
            entry("k_changed", Some("DDDD"), 15000, 1_500_000),
            entry("k_added", Some("EEEE"), 25000, 2_500_000),
        ],
    );

    std::fs::write(&base_file, serde_json::to_string_pretty(&base).unwrap()).unwrap();
    std::fs::write(
        &against_file,
        serde_json::to_string_pretty(&against).unwrap(),
    )
    .unwrap();

    // Run with an unroutable RPC URL to guarantee no network call is attempted.
    let output = sdkt()
        .args([
            "storage",
            "diff",
            base_file.to_str().unwrap(),
            "--against",
            against_file.to_str().unwrap(),
            "--format",
            "json",
            "--rpc-url",
            "http://unroutable.invalid:9999",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let diff: SnapshotDiff = serde_json::from_slice(&output).expect("valid SnapshotDiff JSON");

    assert_eq!(diff.contract_id, TEST_CONTRACT);
    assert_eq!(diff.unchanged, 1);
    assert_eq!(diff.added, vec!["k_added".to_string()]);
    assert_eq!(diff.removed, vec!["k_removed".to_string()]);

    assert_eq!(diff.value_changed.len(), 1);
    assert_eq!(diff.value_changed[0].key, "k_changed");
    assert_eq!(diff.value_changed[0].before.as_deref(), Some("CCCC"));
    assert_eq!(diff.value_changed[0].after.as_deref(), Some("DDDD"));

    assert_eq!(diff.ttl_changed.len(), 1);
    assert_eq!(diff.ttl_changed[0].key, "k_changed");
    assert_eq!(diff.ttl_changed[0].before_ttl, 20000);
    assert_eq!(diff.ttl_changed[0].after_ttl, 15000);
    assert_eq!(diff.ttl_changed[0].before_extension_cost_stroops, 2_000_000);
    assert_eq!(diff.ttl_changed[0].after_extension_cost_stroops, 1_500_000);

    assert_eq!(diff.changed(), 4);
}

#[test]
fn storage_diff_against_pretty_output_layout() {
    let dir = tempdir().unwrap();
    let base_file = dir.path().join("base.json");
    let against_file = dir.path().join("against.json");

    let base = snapshot(
        TEST_CONTRACT,
        vec![
            entry("k_unchanged", Some("AAAA"), 20000, 2_000_000),
            entry("k_removed", Some("BBBB"), 20000, 2_000_000),
            entry("k_changed", Some("CCCC"), 20000, 2_000_000),
        ],
    );

    let against = snapshot(
        TEST_CONTRACT,
        vec![
            entry("k_unchanged", Some("AAAA"), 20000, 2_000_000),
            entry("k_changed", Some("DDDD"), 15000, 1_500_000),
            entry("k_added", Some("EEEE"), 25000, 2_500_000),
        ],
    );

    std::fs::write(&base_file, serde_json::to_string_pretty(&base).unwrap()).unwrap();
    std::fs::write(
        &against_file,
        serde_json::to_string_pretty(&against).unwrap(),
    )
    .unwrap();

    sdkt()
        .args([
            "storage",
            "diff",
            base_file.to_str().unwrap(),
            "--against",
            against_file.to_str().unwrap(),
            "--rpc-url",
            "http://unroutable.invalid:9999",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Storage Diff for Contract: {}",
            TEST_CONTRACT
        )))
        .stdout(predicate::str::contains("Changed:   4"))
        .stdout(predicate::str::contains("Unchanged: 1"))
        .stdout(predicate::str::contains("Value Changed:"))
        .stdout(predicate::str::contains("TTL Changed:"))
        .stdout(predicate::str::contains("Added:"))
        .stdout(predicate::str::contains("Removed:"))
        .stdout(predicate::str::contains("k_added"))
        .stdout(predicate::str::contains("k_removed"))
        .stdout(predicate::str::contains("k_changed"));
}

#[test]
fn storage_diff_against_missing_file_fails_with_clear_error() {
    let dir = tempdir().unwrap();
    let base_file = dir.path().join("base.json");
    let base = snapshot(TEST_CONTRACT, vec![entry("k1", Some("A"), 100, 100)]);
    std::fs::write(&base_file, serde_json::to_string(&base).unwrap()).unwrap();

    let missing = dir.path().join("missing_against.json");

    sdkt()
        .args([
            "storage",
            "diff",
            base_file.to_str().unwrap(),
            "--against",
            missing.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read snapshot"))
        .stderr(predicate::str::contains("missing_against.json"));
}

#[test]
fn storage_diff_against_malformed_file_fails_with_clear_error() {
    let dir = tempdir().unwrap();
    let base_file = dir.path().join("base.json");
    let malformed_file = dir.path().join("malformed.json");

    let base = snapshot(TEST_CONTRACT, vec![entry("k1", Some("A"), 100, 100)]);
    std::fs::write(&base_file, serde_json::to_string(&base).unwrap()).unwrap();
    std::fs::write(&malformed_file, "this is { not valid json").unwrap();

    sdkt()
        .args([
            "storage",
            "diff",
            base_file.to_str().unwrap(),
            "--against",
            malformed_file.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid snapshot document"))
        .stderr(predicate::str::contains("malformed.json"));
}

#[test]
fn storage_diff_absent_against_flag_preserves_live_behavior() {
    // When --against is absent, missing base file still errors offline with cannot read snapshot
    sdkt()
        .args(["storage", "diff", "nonexistent-base-file.json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read snapshot"));
}

#[test]
fn storage_diff_against_missing_base_file_fails_with_clear_error() {
    let dir = tempdir().unwrap();
    let against_file = dir.path().join("against.json");
    let against = snapshot(TEST_CONTRACT, vec![entry("k1", Some("A"), 100, 100)]);
    std::fs::write(&against_file, serde_json::to_string(&against).unwrap()).unwrap();

    let missing = dir.path().join("missing_base.json");

    sdkt()
        .args([
            "storage",
            "diff",
            missing.to_str().unwrap(),
            "--against",
            against_file.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read snapshot"))
        .stderr(predicate::str::contains("missing_base.json"));
}

#[test]
fn storage_diff_against_malformed_base_file_fails_with_clear_error() {
    let dir = tempdir().unwrap();
    let malformed_base = dir.path().join("malformed_base.json");
    let against_file = dir.path().join("against.json");
    let against = snapshot(TEST_CONTRACT, vec![entry("k1", Some("A"), 100, 100)]);

    std::fs::write(&malformed_base, "invalid json content").unwrap();
    std::fs::write(&against_file, serde_json::to_string(&against).unwrap()).unwrap();

    sdkt()
        .args([
            "storage",
            "diff",
            malformed_base.to_str().unwrap(),
            "--against",
            against_file.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid snapshot document"))
        .stderr(predicate::str::contains("malformed_base.json"));
}

#[test]
fn storage_diff_against_legacy_snapshots_without_values_diffs_ttl_only() {
    let dir = tempdir().unwrap();
    let base_file = dir.path().join("base.json");
    let against_file = dir.path().join("against.json");

    // Two snapshots without value field (e.g. legacy captures)
    let base = snapshot(
        TEST_CONTRACT,
        vec![
            entry("k1", None, 20000, 2_000_000),
            entry("k2", None, 20000, 2_000_000),
        ],
    );
    let against = snapshot(
        TEST_CONTRACT,
        vec![
            entry("k1", None, 20000, 2_000_000), // unchanged
            entry("k2", None, 10000, 1_000_000), // ttl changed
        ],
    );

    std::fs::write(&base_file, serde_json::to_string(&base).unwrap()).unwrap();
    std::fs::write(&against_file, serde_json::to_string(&against).unwrap()).unwrap();

    let output = sdkt()
        .args([
            "storage",
            "diff",
            base_file.to_str().unwrap(),
            "--against",
            against_file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let diff: SnapshotDiff = serde_json::from_slice(&output).expect("valid diff");
    assert_eq!(diff.unchanged, 1);
    assert_eq!(diff.ttl_changed.len(), 1);
    assert_eq!(diff.ttl_changed[0].key, "k2");
    assert!(diff.value_changed.is_empty());
    assert!(diff.added.is_empty());
    assert!(diff.removed.is_empty());
}

#[test]
fn storage_diff_against_succeeds_even_with_nonexistent_network_profile() {
    let dir = tempdir().unwrap();
    let base_file = dir.path().join("base.json");
    let against_file = dir.path().join("against.json");

    let base = snapshot(TEST_CONTRACT, vec![entry("k1", Some("valA"), 100, 10)]);
    let against = snapshot(TEST_CONTRACT, vec![entry("k1", Some("valA"), 100, 10)]);

    std::fs::write(&base_file, serde_json::to_string(&base).unwrap()).unwrap();
    std::fs::write(&against_file, serde_json::to_string(&against).unwrap()).unwrap();

    // Specifying a non-existent network profile must not cause an error or profile lookup
    // since offline diff avoids shared RPC/profile resolution entirely.
    sdkt()
        .args([
            "storage",
            "--network-profile",
            "nonexistent-network-profile-12345",
            "diff",
            base_file.to_str().unwrap(),
            "--against",
            against_file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success();
}

#[test]
fn storage_diff_against_rejects_abi_option() {
    let dir = tempdir().unwrap();
    let base_file = dir.path().join("base.json");
    let against_file = dir.path().join("against.json");
    let fake_wasm = dir.path().join("contract.wasm");

    let base = snapshot(TEST_CONTRACT, vec![entry("k1", Some("valA"), 100, 10)]);
    let against = snapshot(TEST_CONTRACT, vec![entry("k1", Some("valA"), 100, 10)]);

    std::fs::write(&base_file, serde_json::to_string(&base).unwrap()).unwrap();
    std::fs::write(&against_file, serde_json::to_string(&against).unwrap()).unwrap();
    std::fs::write(&fake_wasm, b"\0asm...").unwrap();

    sdkt()
        .args([
            "storage",
            "--abi",
            fake_wasm.to_str().unwrap(),
            "diff",
            base_file.to_str().unwrap(),
            "--against",
            against_file.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: --abi and --abi-contract options do not apply to 'storage diff'",
        ));
}

#[test]
fn storage_diff_against_rejects_abi_contract_option() {
    let dir = tempdir().unwrap();
    let base_file = dir.path().join("base.json");
    let against_file = dir.path().join("against.json");

    let base = snapshot(TEST_CONTRACT, vec![entry("k1", Some("valA"), 100, 10)]);
    let against = snapshot(TEST_CONTRACT, vec![entry("k1", Some("valA"), 100, 10)]);

    std::fs::write(&base_file, serde_json::to_string(&base).unwrap()).unwrap();
    std::fs::write(&against_file, serde_json::to_string(&against).unwrap()).unwrap();

    sdkt()
        .args([
            "storage",
            "--abi-contract",
            TEST_CONTRACT,
            "diff",
            base_file.to_str().unwrap(),
            "--against",
            against_file.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: --abi and --abi-contract options do not apply to 'storage diff'",
        ));
}
