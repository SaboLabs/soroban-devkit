//! Storage snapshots and value-aware diffing.
//!
//! A snapshot records, for each tracked ledger key, its storage class,
//! durability, remaining TTL, estimated rent, and — for entries whose value can
//! be read — the captured value as a base64 XDR `ScVal`.
//!
//! Snapshots are plain JSON so an operator can archive one before an upgrade or
//! during an incident and re-run `sdkt storage diff` against a fresh live read.
//! The captured value is optional: a document written before value capture
//! existed still deserializes with `value: None`, and its diff falls back to the
//! TTL/rent comparison only.

use crate::analyzer::classify_key;
use crate::error::StorageError;
use crate::types::StorageClass;
use base64::Engine;
use sdkt_rpc::SorobanRpcClient;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use stellar_xdr::WriteXdr;

/// A snapshot document: the captured storage state of one contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StorageSnapshot {
    /// Contract the snapshot was captured from.
    pub contract_id: String,
    /// Ledger sequence at capture time, when it could be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at_ledger: Option<u32>,
    /// Per-entry detail. Defaulted so a document with no entries still parses.
    #[serde(default)]
    pub entries: Vec<SnapshotEntry>,
}

/// One tracked ledger entry inside a [`StorageSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotEntry {
    /// The base64 XDR `LedgerKey` as returned by RPC.
    pub key: String,
    /// Storage class derived from the ledger key.
    #[serde(default)]
    pub class: StorageClass,
    /// Durability of the entry (`persistent` / `temporary`); absent for
    /// non-contract-data keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability: Option<String>,
    /// Ledgers of TTL remaining at capture time.
    #[serde(default)]
    pub current_ttl: u32,
    /// Estimated rent extension cost in stroops.
    #[serde(default)]
    pub extension_cost_stroops: u64,
    /// Captured entry value as a base64 XDR `ScVal`.
    ///
    /// Optional so snapshots written before value capture existed keep parsing;
    /// serialized only when a value was actually captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// A tracked entry whose captured value changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueDelta {
    pub key: String,
    pub class: StorageClass,
    /// Value in the base snapshot (base64 XDR `ScVal`).
    pub before: Option<String>,
    /// Value in the live snapshot (base64 XDR `ScVal`).
    pub after: Option<String>,
}

/// A tracked entry whose TTL or estimated rent changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TtlDelta {
    pub key: String,
    pub class: StorageClass,
    pub before_ttl: u32,
    pub after_ttl: u32,
    pub before_extension_cost_stroops: u64,
    pub after_extension_cost_stroops: u64,
}

/// The structural difference between a base snapshot and a live snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SnapshotDiff {
    pub contract_id: String,
    /// Tracked entries whose value and TTL both match.
    pub unchanged: usize,
    /// Entries whose captured value changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub value_changed: Vec<ValueDelta>,
    /// Entries whose TTL and/or estimated rent changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ttl_changed: Vec<TtlDelta>,
    /// Keys present in the live snapshot but not in the base snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// Keys present in the base snapshot but not in the live snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

impl SnapshotDiff {
    /// Total number of deltas across all change kinds.
    pub fn changed(&self) -> usize {
        self.value_changed.len() + self.ttl_changed.len() + self.added.len() + self.removed.len()
    }
}

/// Capture a snapshot of a contract's storage.
///
/// `extra_keys` are additional ledger keys (base64/hex XDR) beyond the always
/// tracked contract instance singleton. TTL and rent come from
/// [`sdkt_rpc::get_ttl_info_for_keys`] and the value from
/// [`sdkt_rpc::read_contract_state`] — the same ledger-key capture path the rest
/// of the storage surface uses, so both see identical keys.
pub async fn capture_snapshot(
    client: &SorobanRpcClient,
    contract_id: &str,
    extra_keys: &[String],
) -> Result<StorageSnapshot, StorageError> {
    let ttl_info = sdkt_rpc::get_ttl_info_for_keys(client, contract_id, extra_keys).await?;
    let captured_at_ledger = client.get_ledger().await.ok().map(|info| info.sequence);

    let mut entries = Vec::with_capacity(ttl_info.entries.len());
    for entry in &ttl_info.entries {
        let class = classify_key(&entry.key);
        let value = match sdkt_rpc::read_contract_state(client, contract_id, &entry.key, None).await
        {
            Ok(state) => state
                .value
                .get("xdr")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            // Non-contract-data keys (accounts, code) carry no `ScVal` value to
            // capture. Every other read failure is a real error and must not be
            // silently recorded as "no value".
            Err(_) if class == StorageClass::Other => None,
            Err(e) => return Err(e.into()),
        };

        entries.push(SnapshotEntry {
            key: entry.key.clone(),
            class,
            durability: durability_label(class),
            current_ttl: entry.current_ttl,
            extension_cost_stroops: entry.extension_cost_stroops,
            value,
        });
    }

    Ok(StorageSnapshot {
        contract_id: contract_id.to_string(),
        captured_at_ledger,
        entries,
    })
}

fn durability_label(class: StorageClass) -> Option<String> {
    match class {
        StorageClass::Instance | StorageClass::Persistent => Some("persistent".to_string()),
        StorageClass::Temporary => Some("temporary".to_string()),
        StorageClass::Other => None,
    }
}

/// Compare a base snapshot against a live snapshot.
///
/// Value and TTL are compared independently: a value-only change produces a
/// [`ValueDelta`], a TTL/rent-only change produces a [`TtlDelta`], and an entry
/// where both match counts as unchanged. A value is only compared when both
/// sides captured one, so snapshots written before value capture existed keep
/// their original TTL/rent-only diff semantics.
pub fn diff_snapshots(base: &StorageSnapshot, live: &StorageSnapshot) -> SnapshotDiff {
    let base_entries = index_entries(&base.entries);
    let live_entries = index_entries(&live.entries);

    let mut diff = SnapshotDiff {
        contract_id: base.contract_id.clone(),
        ..Default::default()
    };

    for (key, before) in &base_entries {
        let Some(after) = live_entries.get(key) else {
            diff.removed.push(key.clone());
            continue;
        };

        let mut changed = false;

        if let (Some(before_value), Some(after_value)) = (&before.value, &after.value) {
            if before_value != after_value {
                diff.value_changed.push(ValueDelta {
                    key: key.clone(),
                    class: after.class,
                    before: before.value.clone(),
                    after: after.value.clone(),
                });
                changed = true;
            }
        }

        if before.current_ttl != after.current_ttl
            || before.extension_cost_stroops != after.extension_cost_stroops
        {
            diff.ttl_changed.push(TtlDelta {
                key: key.clone(),
                class: after.class,
                before_ttl: before.current_ttl,
                after_ttl: after.current_ttl,
                before_extension_cost_stroops: before.extension_cost_stroops,
                after_extension_cost_stroops: after.extension_cost_stroops,
            });
            changed = true;
        }

        if !changed {
            diff.unchanged += 1;
        }
    }

    for key in live_entries.keys() {
        if !base_entries.contains_key(key) {
            diff.added.push(key.clone());
        }
    }

    diff
}

/// Index entries by their canonical base64 `LedgerKey`, so a key expressed as
/// hex in one document and base64 in the other still compares equal.
fn index_entries(entries: &[SnapshotEntry]) -> BTreeMap<String, &SnapshotEntry> {
    let mut map = BTreeMap::new();
    for entry in entries {
        map.insert(canonical_key(&entry.key), entry);
    }
    map
}

fn canonical_key(key: &str) -> String {
    let trimmed = key.trim();
    let Ok(decoded) = sdkt_xdr::decode_ledger_key(trimmed) else {
        return trimmed.to_string();
    };
    let mut buf = Vec::new();
    let mut limited = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    if decoded.write_xdr(&mut limited).is_err() {
        return trimmed.to_string();
    }
    base64::engine::general_purpose::STANDARD.encode(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use stellar_xdr::{
        ContractDataDurability, ContractDataEntry, ContractId, ExtensionPoint, Hash, LedgerEntry,
        LedgerEntryData, LedgerEntryExt, ScAddress, ScVal,
    };

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

    fn snapshot(entries: Vec<SnapshotEntry>) -> StorageSnapshot {
        StorageSnapshot {
            contract_id: TEST_CONTRACT.to_string(),
            captured_at_ledger: Some(1000),
            entries,
        }
    }

    #[test]
    fn value_only_change_is_a_value_delta_not_ttl() {
        let base = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("BBBB"), 20000, 2_000_000)]);

        let diff = diff_snapshots(&base, &live);

        assert_eq!(diff.changed(), 1);
        assert_eq!(diff.unchanged, 0);
        assert!(diff.ttl_changed.is_empty(), "TTL matched, so no TTL delta");
        assert_eq!(diff.value_changed.len(), 1);
        let delta = &diff.value_changed[0];
        assert_eq!(delta.key, "k1");
        assert_eq!(delta.before.as_deref(), Some("AAAA"));
        assert_eq!(delta.after.as_deref(), Some("BBBB"));
    }

    #[test]
    fn matching_value_and_ttl_is_unchanged() {
        let base = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);

        let diff = diff_snapshots(&base, &live);

        assert_eq!(diff.changed(), 0);
        assert_eq!(diff.unchanged, 1);
    }

    #[test]
    fn ttl_only_change_reports_only_ttl_delta() {
        let base = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("AAAA"), 15000, 1_500_000)]);

        let diff = diff_snapshots(&base, &live);

        assert_eq!(diff.changed(), 1);
        assert!(
            diff.value_changed.is_empty(),
            "value matched, so no value delta"
        );
        assert_eq!(diff.ttl_changed.len(), 1);
        let delta = &diff.ttl_changed[0];
        assert_eq!(delta.before_ttl, 20000);
        assert_eq!(delta.after_ttl, 15000);
        assert_eq!(delta.before_extension_cost_stroops, 2_000_000);
        assert_eq!(delta.after_extension_cost_stroops, 1_500_000);
    }

    #[test]
    fn rent_change_without_ttl_change_is_a_ttl_delta() {
        let base = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("AAAA"), 20000, 2_500_000)]);

        let diff = diff_snapshots(&base, &live);

        assert!(diff.value_changed.is_empty());
        assert_eq!(diff.ttl_changed.len(), 1);
        assert_eq!(diff.unchanged, 0);
    }

    #[test]
    fn value_less_snapshot_keeps_ttl_only_semantics() {
        // A document written before value capture existed has no `value` field.
        let base = snapshot(vec![entry("k1", None, 20000, 2_000_000)]);
        let live = snapshot(vec![entry("k1", Some("BBBB"), 20000, 2_000_000)]);

        let diff = diff_snapshots(&base, &live);

        assert_eq!(
            diff.changed(),
            0,
            "with no captured base value there is nothing to compare"
        );
        assert_eq!(diff.unchanged, 1);
    }

    #[test]
    fn added_and_removed_keys_are_reported() {
        let base = snapshot(vec![
            entry("gone", Some("AAAA"), 10, 1000),
            entry("same", None, 5, 500),
        ]);
        let live = snapshot(vec![
            entry("same", None, 5, 500),
            entry("fresh", None, 7, 700),
        ]);

        let diff = diff_snapshots(&base, &live);

        assert_eq!(diff.removed, vec!["gone".to_string()]);
        assert_eq!(diff.added, vec!["fresh".to_string()]);
        assert_eq!(diff.unchanged, 1);
        assert_eq!(diff.changed(), 2);
    }

    #[test]
    fn keys_are_compared_by_canonical_form() {
        // Same ledger key expressed as base64 in the base snapshot and as raw
        // base64 in the live snapshot must match.
        let key = sdkt_xdr::encode_ledger_key(&sdkt_xdr::LedgerKeyParams::ContractDataEntry {
            contract: TEST_CONTRACT.to_string(),
            key: ScVal::U32(1),
            durability: ContractDataDurability::Persistent,
        })
        .unwrap();

        let base = snapshot(vec![entry(&key, Some("AAAA"), 10, 1000)]);
        let live = snapshot(vec![entry(&format!("  {key}  "), Some("BBBB"), 10, 1000)]);

        let diff = diff_snapshots(&base, &live);

        assert_eq!(diff.value_changed.len(), 1);
        assert_eq!(diff.removed.len(), 0);
        assert_eq!(diff.added.len(), 0);
    }

    #[test]
    fn value_less_document_deserializes_with_value_none() {
        // Snapshot JSON written before this change: no `value` key present.
        let json = format!(
            r#"{{"contract_id":"{TEST_CONTRACT}","entries":[
                {{"key":"some-key","class":"persistent","durability":"persistent",
                  "current_ttl":20000,"extension_cost_stroops":2000000}}
            ]}}"#
        );

        let snapshot: StorageSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].value, None);
        assert_eq!(snapshot.entries[0].current_ttl, 20000);

        // A minimal document with only a key also parses (defaulted fields).
        let minimal: StorageSnapshot = serde_json::from_str(&format!(
            r#"{{"contract_id":"{TEST_CONTRACT}","entries":[{{"key":"k"}}]}}"#
        ))
        .unwrap();
        assert_eq!(minimal.entries[0].value, None);
        assert_eq!(minimal.entries[0].current_ttl, 0);
    }

    #[test]
    fn captured_value_is_serialized_only_when_present() {
        let with_value = snapshot(vec![entry("k1", Some("AAAA"), 10, 100)]);
        let json = serde_json::to_string(&with_value).unwrap();
        assert!(json.contains(r#""value":"AAAA""#));

        let without_value = snapshot(vec![entry("k1", None, 10, 100)]);
        let json = serde_json::to_string(&without_value).unwrap();
        assert!(!json.contains("\"value\""));
    }

    /// Encode an `ScVal` as the base64 XDR string `read_contract_state` returns.
    fn scval_base64(val: &ScVal) -> String {
        let mut buf = Vec::new();
        let mut limited = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
        val.write_xdr(&mut limited).unwrap();
        base64::engine::general_purpose::STANDARD.encode(&buf)
    }

    /// Encode a `ContractData` `LedgerEntry` whose captured value is `val`.
    fn contract_data_entry_xdr(val: ScVal) -> String {
        let ledger_entry = LedgerEntry {
            last_modified_ledger_seq: 100,
            data: LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::U32(1),
                durability: ContractDataDurability::Persistent,
                val,
            }),
            ext: LedgerEntryExt::V0,
        };
        let mut buf = Vec::new();
        let mut limited = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
        ledger_entry.write_xdr(&mut limited).unwrap();
        base64::engine::general_purpose::STANDARD.encode(&buf)
    }

    fn contract_data_key(key: ScVal) -> String {
        sdkt_xdr::encode_ledger_key(&sdkt_xdr::LedgerKeyParams::ContractDataEntry {
            contract: TEST_CONTRACT.to_string(),
            key,
            durability: ContractDataDurability::Persistent,
        })
        .unwrap()
    }

    /// Read one HTTP request in full (headers + body per Content-Length).
    fn read_request(sock: &mut std::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = sock.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&data).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text[..end]
                    .lines()
                    .filter_map(|l| l.split_once(':'))
                    .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if data.len() >= end + 4 + len {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&data).to_string()
    }

    /// Mock RPC that echoes one entry per requested key, taking each entry's
    /// captured value from `values` (default 100) and a fixed TTL.
    fn mock_snapshot_rpc(
        values: Arc<Mutex<BTreeMap<String, u32>>>,
    ) -> (String, Arc<Mutex<Vec<Vec<String>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let queried: Arc<Mutex<Vec<Vec<String>>>> = Default::default();
        let queried_thread = queried.clone();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut sock) = conn else { break };
                let req = read_request(&mut sock);
                let body = req.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
                let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
                let method = parsed
                    .as_ref()
                    .and_then(|v| v["method"].as_str())
                    .unwrap_or_default()
                    .to_string();

                let resp_body = match method.as_str() {
                    "getLatestLedger" => {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":22,"sequence":1200}}"#
                            .to_string()
                    }
                    "getLedgerEntries" => {
                        let keys: Vec<String> = parsed
                            .as_ref()
                            .and_then(|v| v["params"]["keys"].as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|k| k.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        queried_thread.lock().unwrap().push(keys.clone());

                        let values = values.lock().unwrap();
                        let entries: Vec<serde_json::Value> = keys
                            .iter()
                            .map(|k| {
                                let val = *values.get(k).unwrap_or(&100);
                                serde_json::json!({
                                    "key": k,
                                    "xdr": contract_data_entry_xdr(ScVal::U32(val)),
                                    "lastModifiedLedgerSeq": 1000,
                                    "liveUntilLedgerSeq": 21200,
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "result": { "entries": entries, "latestLedger": 1200 }
                        })
                        .to_string()
                    }
                    _ => r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not found"}}"#
                        .to_string(),
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        (url, queried)
    }

    #[tokio::test]
    async fn capture_snapshot_records_each_entry_value() {
        let persistent_key = contract_data_key(ScVal::U32(10));
        let instance_key = contract_data_key(ScVal::LedgerKeyContractInstance);

        let mut initial = BTreeMap::new();
        initial.insert(persistent_key.clone(), 4242);
        initial.insert(instance_key.clone(), 7);
        let values = Arc::new(Mutex::new(initial));

        let (url, _queried) = mock_snapshot_rpc(values.clone());
        let client = SorobanRpcClient::new(&url);

        let snapshot = capture_snapshot(
            &client,
            TEST_CONTRACT,
            std::slice::from_ref(&persistent_key),
        )
        .await
        .unwrap();

        assert_eq!(snapshot.contract_id, TEST_CONTRACT);
        assert_eq!(snapshot.captured_at_ledger, Some(1200));
        assert_eq!(snapshot.entries.len(), 2);

        let persistent = snapshot
            .entries
            .iter()
            .find(|e| e.key == persistent_key)
            .expect("persistent entry captured");
        assert_eq!(persistent.class, StorageClass::Persistent);
        assert_eq!(persistent.durability.as_deref(), Some("persistent"));
        assert_eq!(persistent.current_ttl, 20000);
        assert_eq!(
            persistent.value.as_deref(),
            Some(scval_base64(&ScVal::U32(4242)).as_str())
        );

        let instance = snapshot
            .entries
            .iter()
            .find(|e| e.key == instance_key)
            .expect("instance entry captured");
        assert_eq!(instance.class, StorageClass::Instance);
    }

    #[tokio::test]
    async fn capture_then_diff_reports_value_only_change() {
        let persistent_key = contract_data_key(ScVal::U32(10));
        let instance_key = contract_data_key(ScVal::LedgerKeyContractInstance);

        let mut initial = BTreeMap::new();
        initial.insert(persistent_key.clone(), 100);
        initial.insert(instance_key.clone(), 7);
        let values = Arc::new(Mutex::new(initial));

        let (url, _queried) = mock_snapshot_rpc(values.clone());
        let client = SorobanRpcClient::new(&url);

        let base = capture_snapshot(
            &client,
            TEST_CONTRACT,
            std::slice::from_ref(&persistent_key),
        )
        .await
        .unwrap();

        // Only the persistent entry's value changes; its TTL and the whole
        // instance entry stay identical.
        values.lock().unwrap().insert(persistent_key.clone(), 0);

        let live = capture_snapshot(
            &client,
            TEST_CONTRACT,
            std::slice::from_ref(&persistent_key),
        )
        .await
        .unwrap();

        let diff = diff_snapshots(&base, &live);

        assert_eq!(diff.changed(), 1, "{diff:?}");
        assert_eq!(diff.unchanged, 1);
        assert!(diff.ttl_changed.is_empty());
        assert_eq!(diff.value_changed.len(), 1);
        assert_eq!(diff.value_changed[0].key, persistent_key);
        assert_eq!(
            diff.value_changed[0].before.as_deref(),
            Some(scval_base64(&ScVal::U32(100)).as_str())
        );
        assert_eq!(
            diff.value_changed[0].after.as_deref(),
            Some(scval_base64(&ScVal::U32(0)).as_str())
        );
    }
}
