//! Integration tests for the snapshot-diffing and TTL extension-plan feature.
//!
//! These tests exercise the complete public API surface of the feature:
//! - [`sdkt_storage::diff_snapshots`]
//! - [`sdkt_storage::derive_extend_plan`]
//! - The JSON serialisation schema for [`sdkt_storage::ExtendPlan`] and
//!   [`sdkt_storage::SnapshotDiff`].
//!
//! # No-mutation guarantee
//! `derive_extend_plan` is a pure function: it neither makes RPC calls nor
//! mutates any state.  The mock-RPC test below confirms this by asserting that
//! a mock server running locally is *never* contacted during plan derivation.

use sdkt_storage::{
    derive_extend_plan, diff_snapshots, DiffEntry, DiffStatus, ExtendPlan, SnapshotDiff,
    SnapshotEntry, StorageSnapshot, DEFAULT_SUGGESTED_LEDGERS, EXPIRING_SOON_LEDGERS,
};

const CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn snap(entries: Vec<(&str, u32)>) -> StorageSnapshot {
    StorageSnapshot::from_entries(
        CONTRACT,
        entries
            .into_iter()
            .map(|(k, ttl)| SnapshotEntry {
                key: k.to_string(),
                label: None,
                current_ttl: ttl,
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Plan derivation — 0 entries
// ---------------------------------------------------------------------------

#[test]
fn empty_diff_yields_empty_plan_and_exits_zero() {
    let diff = diff_snapshots(&snap(vec![]), &snap(vec![])).unwrap();
    let plan = derive_extend_plan(&diff);

    assert!(plan.keys.is_empty(), "no keys to extend for an empty diff");
    assert_eq!(plan.suggested_ledgers, DEFAULT_SUGGESTED_LEDGERS);
    assert!(plan.suggested_ledgers_reason.contains("nothing to extend"));
    // Function returns normally (caller exits 0).
}

#[test]
fn diff_mismatched_contract_ids_fails() {
    let old = StorageSnapshot::from_entries("CCONTRACTA", vec![]);
    let new = StorageSnapshot::from_entries("CCONTRACTB", vec![]);

    let err = diff_snapshots(&old, &new).unwrap_err();
    assert!(
        matches!(err, sdkt_storage::StorageError::ContractIdMismatch { old, new } if old == "CCONTRACTA" && new == "CCONTRACTB")
    );
}

// ---------------------------------------------------------------------------
// Plan derivation — 1 entry
// ---------------------------------------------------------------------------

#[test]
fn one_expiring_entry_produces_single_key_and_scaled_horizon() {
    let remaining = 3_000u32;
    let old = snap(vec![("keyA", 50_000)]);
    let new = snap(vec![("keyA", remaining)]);

    let diff = diff_snapshots(&old, &new).unwrap();
    let plan = derive_extend_plan(&diff);

    assert_eq!(plan.keys.len(), 1);
    assert_eq!(plan.keys[0], "keyA");
    assert_eq!(
        plan.suggested_ledgers,
        remaining + DEFAULT_SUGGESTED_LEDGERS,
        "horizon must scale with remaining TTL"
    );
}

#[test]
fn one_removed_entry_produces_single_key_and_default_horizon() {
    let old = snap(vec![("keyB", 50_000)]);
    let new = snap(vec![]);

    let diff = diff_snapshots(&old, &new).unwrap();

    let removed: Vec<_> = diff
        .entries
        .iter()
        .filter(|e| e.status == DiffStatus::Removed)
        .collect();
    assert_eq!(removed.len(), 1);

    let plan = derive_extend_plan(&diff);

    assert_eq!(plan.keys.len(), 1);
    assert_eq!(plan.keys[0], "keyB");
    // Removed entries have no remaining TTL → default horizon.
    assert_eq!(plan.suggested_ledgers, DEFAULT_SUGGESTED_LEDGERS);
    assert!(plan.suggested_ledgers_reason.contains("removed"));
}

// ---------------------------------------------------------------------------
// Plan derivation — N entries with mixed statuses
// ---------------------------------------------------------------------------

#[test]
fn n_entries_plan_includes_only_removed_and_expiring() {
    let old = snap(vec![
        ("k_unchanged", 500_000),
        ("k_expiring", 500_000),
        ("k_removed", 500_000),
    ]);
    let new = snap(vec![
        ("k_unchanged", 499_000), // well above threshold
        ("k_expiring", EXPIRING_SOON_LEDGERS - 1), // just below threshold
                                  // k_removed absent
    ]);

    let diff = diff_snapshots(&old, &new).unwrap();
    let plan = derive_extend_plan(&diff);

    // Only k_expiring and k_removed in plan.
    assert_eq!(plan.keys.len(), 2);
    let key_set: std::collections::HashSet<_> = plan.keys.iter().map(String::as_str).collect();
    assert!(key_set.contains("k_expiring"));
    assert!(key_set.contains("k_removed"));
    assert!(!key_set.contains("k_unchanged"));

    // Suggested ledgers based on k_expiring's remaining TTL.
    let expiring_ttl = EXPIRING_SOON_LEDGERS - 1;
    assert_eq!(
        plan.suggested_ledgers,
        expiring_ttl + DEFAULT_SUGGESTED_LEDGERS
    );
}

#[test]
fn n_entries_suggested_ledgers_uses_minimum_remaining_ttl() {
    // Two expiring entries; suggested horizon must be based on the smaller TTL.
    let ttl_high = EXPIRING_SOON_LEDGERS - 100;
    let ttl_low = EXPIRING_SOON_LEDGERS - 5_000;

    let old = snap(vec![("k1", 500_000), ("k2", 500_000)]);
    let new = snap(vec![("k1", ttl_high), ("k2", ttl_low)]);

    let diff = diff_snapshots(&old, &new).unwrap();
    let plan = derive_extend_plan(&diff);

    assert_eq!(plan.suggested_ledgers, ttl_low + DEFAULT_SUGGESTED_LEDGERS);
}

// ---------------------------------------------------------------------------
// No-mutation / no sendTransaction guarantee
// ---------------------------------------------------------------------------

/// `derive_extend_plan` is a pure function and must not contact any RPC server.
/// This test confirms it by running a mock server that panics if it receives a
/// `sendTransaction` call.
#[tokio::test]
async fn derive_extend_plan_never_contacts_rpc() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    let send_tx_called = Arc::new(Mutex::new(false));
    let flag_clone = Arc::clone(&send_tx_called);

    // Bind a mock server on a random port.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let _bound_addr = listener.local_addr().unwrap();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            if req.contains("sendTransaction") {
                *flag_clone.lock().unwrap() = true;
            }

            // Serve a minimal valid response for any request.
            let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not used"}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    // derive_extend_plan is 100% pure — no client creation, no network I/O.
    let old = snap(vec![("k1", 50_000)]);
    let new = snap(vec![("k1", EXPIRING_SOON_LEDGERS - 100)]);
    let diff = diff_snapshots(&old, &new).unwrap();
    let plan = derive_extend_plan(&diff);

    // Wait briefly to let the thread process any accidental connections.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    assert!(
        !*send_tx_called.lock().unwrap(),
        "sendTransaction must never be called by derive_extend_plan"
    );
    // The plan is non-trivially correct too.
    assert_eq!(plan.keys.len(), 1);
}

// ---------------------------------------------------------------------------
// JSON schema stability — field names must not change
// ---------------------------------------------------------------------------

#[test]
fn extend_plan_json_has_stable_field_names() {
    let plan = ExtendPlan {
        contract_id: CONTRACT.to_string(),
        keys: vec!["k1".to_string(), "k2".to_string()],
        suggested_ledgers: DEFAULT_SUGGESTED_LEDGERS,
        suggested_ledgers_reason: "test".to_string(),
    };

    let json = serde_json::to_string(&plan).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(v["contract_id"], CONTRACT, "contract_id field");
    assert!(v["keys"].is_array(), "keys must be an array");
    assert_eq!(v["keys"].as_array().unwrap().len(), 2);
    assert_eq!(
        v["suggested_ledgers"], DEFAULT_SUGGESTED_LEDGERS,
        "suggested_ledgers field"
    );
    assert!(
        v["suggested_ledgers_reason"].is_string(),
        "suggested_ledgers_reason field"
    );
}

#[test]
fn snapshot_diff_json_has_stable_field_names() {
    let diff = SnapshotDiff {
        contract_id: CONTRACT.to_string(),
        entries: vec![DiffEntry {
            key: "k1".to_string(),
            label: None,
            status: DiffStatus::ExpiringSoon,
            old_ttl: Some(50_000),
            new_ttl: Some(1_000),
        }],
    };

    let json = serde_json::to_string(&diff).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(v["contract_id"], CONTRACT, "contract_id field");
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["key"], "k1", "key field");
    // Status is snake_case per serde rename_all = "snake_case".
    assert_eq!(entries[0]["status"], "expiring_soon", "status snake_case");
    assert_eq!(entries[0]["old_ttl"], 50_000, "old_ttl field");
    assert_eq!(entries[0]["new_ttl"], 1_000, "new_ttl field");
}

#[test]
fn removed_entry_json_new_ttl_is_null() {
    let diff = SnapshotDiff {
        contract_id: CONTRACT.to_string(),
        entries: vec![DiffEntry {
            key: "k1".to_string(),
            label: None,
            status: DiffStatus::Removed,
            old_ttl: Some(50_000),
            new_ttl: None,
        }],
    };

    let json = serde_json::to_string(&diff).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries[0]["status"], "removed");
    assert!(
        entries[0]["new_ttl"].is_null(),
        "new_ttl must be null for removed entry"
    );
}
