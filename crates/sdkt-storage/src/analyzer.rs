use crate::error::StorageError;
use crate::types::{StorageClass, StorageEntry, StorageReport, TtlInfoSummary};
use base64::Engine;
use sdkt_rpc::SorobanRpcClient;
use stellar_xdr::{ContractDataDurability, LedgerKey, ReadXdr, ScVal};

/// Threshold (in ledgers) below which an entry is considered "expiring soon".
/// ~1 day at 5s/ledger (17280 ledgers).
const EXPIRING_SOON_LEDGERS: u32 = 17280;

/// Classify a storage entry from its base64 XDR `LedgerKey`.
///
/// - `LedgerKey::ContractData` whose `key` is `ScVal::LedgerKeyContractInstance`
///   is the contract **instance** singleton.
/// - Other `LedgerKey::ContractData` entries are categorized by their
///   `durability` (`Persistent` / `Temporary`).
/// - Anything else (account, trustline, contract code, etc.) is `Other`.
///
/// Returns `StorageClass::Other` if the key cannot be decoded — never errors,
/// so a single malformed entry does not abort the whole analysis.
pub fn classify_key(base64_key: &str) -> StorageClass {
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(base64_key.trim()) else {
        return StorageClass::Other;
    };
    let mut cursor = std::io::Cursor::new(bytes);
    let mut limited = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    let Ok(key) = LedgerKey::read_xdr(&mut limited) else {
        return StorageClass::Other;
    };

    match key {
        LedgerKey::ContractData(cd) => {
            if matches!(cd.key, ScVal::LedgerKeyContractInstance) {
                StorageClass::Instance
            } else {
                match cd.durability {
                    ContractDataDurability::Temporary => StorageClass::Temporary,
                    ContractDataDurability::Persistent => StorageClass::Persistent,
                }
            }
        }
        _ => StorageClass::Other,
    }
}

pub struct StorageAnalyzer {
    client: SorobanRpcClient,
}

impl StorageAnalyzer {
    pub fn new(client: SorobanRpcClient) -> Self {
        Self { client }
    }

    pub async fn inspect_contract_storage(
        &self,
        contract_id: &str,
    ) -> Result<StorageReport, StorageError> {
        self.inspect_contract_storage_keys(contract_id, &[]).await
    }

    pub async fn inspect_contract_storage_keys(
        &self,
        contract_id: &str,
        extra_keys: &[String],
    ) -> Result<StorageReport, StorageError> {
        if contract_id.is_empty() {
            return Err(StorageError::InvalidContractId(
                "Contract ID cannot be empty".to_string(),
            ));
        }

        let ttl_info =
            sdkt_rpc::get_ttl_info_for_keys(&self.client, contract_id, extra_keys).await?;

        if ttl_info.entries.is_empty() {
            return Ok(StorageReport {
                contract_id: contract_id.to_string(),
                ..Default::default()
            });
        }

        let mut min_ttl = u32::MAX;
        let mut max_ttl = 0u32;
        let mut total_ttl: u64 = 0;
        let mut expiring_soon = 0;
        let mut total_cost: u64 = 0;
        let mut ttl_bearing_entries = 0usize;

        let mut instance_entries = 0;
        let mut persistent_entries = 0;
        let mut temporary_entries = 0;
        let mut other_entries = 0;
        let mut detailed: Vec<StorageEntry> = Vec::with_capacity(ttl_info.entries.len());

        for entry in &ttl_info.entries {
            let class = classify_key(&entry.key);
            match class {
                StorageClass::Instance => instance_entries += 1,
                StorageClass::Persistent => persistent_entries += 1,
                StorageClass::Temporary => temporary_entries += 1,
                StorageClass::Other => other_entries += 1,
            }

            if class != StorageClass::Other {
                let ttl = entry.current_ttl;
                if ttl < min_ttl {
                    min_ttl = ttl;
                }
                if ttl > max_ttl {
                    max_ttl = ttl;
                }
                total_ttl += ttl as u64;

                if ttl < EXPIRING_SOON_LEDGERS {
                    expiring_soon += 1;
                }
                total_cost += entry.extension_cost_stroops;
                ttl_bearing_entries += 1;
            }

            detailed.push(StorageEntry {
                key: entry.key.clone(),
                class,
                current_ttl: entry.current_ttl,
                days_remaining: entry.days_remaining,
                extension_cost_stroops: entry.extension_cost_stroops,
            });
        }

        let count = ttl_info.entries.len();
        let ttl_summary = if ttl_bearing_entries > 0 {
            let average_ttl = (total_ttl / ttl_bearing_entries as u64) as u32;
            let minimum_ttl = if min_ttl == u32::MAX { 0 } else { min_ttl };
            Some(TtlInfoSummary {
                minimum_ttl,
                maximum_ttl: max_ttl,
                average_ttl,
                expiring_entries_count: expiring_soon,
                estimated_rent_cost: Some(total_cost),
            })
        } else {
            None
        };

        Ok(StorageReport {
            contract_id: contract_id.to_string(),
            total_entries: count,
            instance_entries,
            persistent_entries,
            temporary_entries,
            other_entries,
            total_size_bytes: None,
            ttl_summary,
            entries: detailed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkt_rpc::SorobanRpcClient;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use stellar_xdr::{
        AccountId, ContractDataDurability, LedgerKey, LedgerKeyAccount, LedgerKeyContractData,
        PublicKey, ScAddress, ScVal, Uint256, WriteXdr,
    };

    const TEST_CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

    fn encode_ledger_key(key: &LedgerKey) -> String {
        let mut buf = Vec::new();
        let mut limited = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
        key.write_xdr(&mut limited).unwrap();
        base64::engine::general_purpose::STANDARD.encode(&buf)
    }

    fn contract_address() -> ScAddress {
        // All-zero contract address (valid XDR shape, value irrelevant for classification).
        ScAddress::Contract(stellar_xdr::ContractId(stellar_xdr::Hash([0u8; 32])))
    }

    #[test]
    fn test_classify_instance() {
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address(),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        });
        assert_eq!(
            classify_key(&encode_ledger_key(&key)),
            StorageClass::Instance
        );
    }

    #[test]
    fn test_classify_persistent() {
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address(),
            key: ScVal::U32(1),
            durability: ContractDataDurability::Persistent,
        });
        assert_eq!(
            classify_key(&encode_ledger_key(&key)),
            StorageClass::Persistent
        );
    }

    #[test]
    fn test_classify_temporary() {
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address(),
            key: ScVal::U32(2),
            durability: ContractDataDurability::Temporary,
        });
        assert_eq!(
            classify_key(&encode_ledger_key(&key)),
            StorageClass::Temporary
        );
    }

    #[test]
    fn test_classify_invalid_base64_is_other() {
        assert_eq!(classify_key("not-valid-base64!!!"), StorageClass::Other);
    }

    #[test]
    fn test_storage_class_label() {
        assert_eq!(StorageClass::Instance.label(), "instance");
        assert_eq!(StorageClass::Persistent.label(), "persistent");
        assert_eq!(StorageClass::Temporary.label(), "temporary");
        assert_eq!(StorageClass::Other.label(), "other");
    }

    #[tokio::test]
    async fn test_inspect_empty_contract_id() {
        let client = SorobanRpcClient::new("http://localhost");
        let analyzer = StorageAnalyzer::new(client);
        let err = analyzer.inspect_contract_storage("").await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidContractId(_)));
    }

    #[tokio::test]
    async fn test_inspect_contract_storage_keys_rejects_invalid_key_offline() {
        let client = SorobanRpcClient::new("http://127.0.0.1:1");
        let analyzer = StorageAnalyzer::new(client);
        let err = analyzer
            .inspect_contract_storage_keys(TEST_CONTRACT, &["invalid-key".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::Rpc(_)));
    }

    #[tokio::test]
    async fn test_merged_classification_multiple_classes() {
        let instance_key = encode_ledger_key(&LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address(),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        }));
        let persistent_key = encode_ledger_key(&LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address(),
            key: ScVal::U32(1),
            durability: ContractDataDurability::Persistent,
        }));
        let temporary_key = encode_ledger_key(&LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address(),
            key: ScVal::U32(2),
            durability: ContractDataDurability::Temporary,
        }));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let ik = instance_key.clone();
        let pk = persistent_key.clone();
        let tk = temporary_key.clone();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut sock) = conn else { break };
                let mut buf = [0u8; 16384];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.contains("getLatestLedger") {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","sequence":100}}"#.to_string()
                } else if req.contains("getLedgerEntries") {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[
                            {{"key":"{ik}","xdr":"AAAAAQAAAABpc25nAAAA","lastModifiedLedgerSeq":100,"liveUntilLedgerSeq":200}},
                            {{"key":"{pk}","xdr":"AAAAAQAAAABpc25nAAAA","lastModifiedLedgerSeq":100,"liveUntilLedgerSeq":300}},
                            {{"key":"{tk}","xdr":"AAAAAQAAAABpc25nAAAA","lastModifiedLedgerSeq":100,"liveUntilLedgerSeq":400}}
                        ],"latestLedger":100}}}}"#
                    )
                } else {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not found"}}"#
                        .to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let client = SorobanRpcClient::new(&url);
        let analyzer = StorageAnalyzer::new(client);

        let report = analyzer
            .inspect_contract_storage_keys(
                TEST_CONTRACT,
                &[persistent_key.clone(), temporary_key.clone()],
            )
            .await
            .unwrap();

        assert_eq!(report.total_entries, 3);
        assert_eq!(report.instance_entries, 1);
        assert_eq!(report.persistent_entries, 1);
        assert_eq!(report.temporary_entries, 1);
        assert_eq!(report.other_entries, 0);

        assert_eq!(report.entries.len(), 3);
        assert_eq!(report.entries[0].class, StorageClass::Instance);
        assert_eq!(report.entries[0].current_ttl, 100);
        assert_eq!(report.entries[1].class, StorageClass::Persistent);
        assert_eq!(report.entries[1].current_ttl, 200);
        assert_eq!(report.entries[2].class, StorageClass::Temporary);
        assert_eq!(report.entries[2].current_ttl, 300);

        let summary = report.ttl_summary.unwrap();
        assert_eq!(summary.minimum_ttl, 100);
        assert_eq!(summary.maximum_ttl, 300);
        assert_eq!(summary.average_ttl, 200);
    }

    #[tokio::test]
    async fn test_inspect_contract_storage_no_keys_instance_only() {
        let instance_key = encode_ledger_key(&LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address(),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        }));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let ik = instance_key.clone();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut sock) = conn else { break };
                let mut buf = [0u8; 16384];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.contains("getLatestLedger") {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","sequence":100}}"#.to_string()
                } else if req.contains("getLedgerEntries") {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[
                            {{"key":"{ik}","xdr":"AAAAAQAAAABpc25nAAAA","lastModifiedLedgerSeq":100,"liveUntilLedgerSeq":200}}
                        ],"latestLedger":100}}}}"#
                    )
                } else {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not found"}}"#
                        .to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let client = SorobanRpcClient::new(&url);
        let analyzer = StorageAnalyzer::new(client);

        let report = analyzer
            .inspect_contract_storage(TEST_CONTRACT)
            .await
            .unwrap();

        assert_eq!(report.total_entries, 1);
        assert_eq!(report.instance_entries, 1);
        assert_eq!(report.persistent_entries, 0);
        assert_eq!(report.temporary_entries, 0);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].class, StorageClass::Instance);
    }

    #[tokio::test]
    async fn test_other_entries_excluded_from_ttl_aggregates() {
        let instance_key = encode_ledger_key(&LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address(),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        }));
        let account_key = encode_ledger_key(&LedgerKey::Account(LedgerKeyAccount {
            account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([1u8; 32]))),
        }));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let ik = instance_key.clone();
        let ak = account_key.clone();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut sock) = conn else { break };
                let mut buf = [0u8; 16384];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.contains("getLatestLedger") {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","sequence":100}}"#.to_string()
                } else if req.contains("getLedgerEntries") {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[
                            {{"key":"{ik}","xdr":"AAAAAQAAAABpc25nAAAA","lastModifiedLedgerSeq":100,"liveUntilLedgerSeq":20100}},
                            {{"key":"{ak}","xdr":"AAAAAQAAAABpc25nAAAA","lastModifiedLedgerSeq":100}}
                        ],"latestLedger":100}}}}"#
                    )
                } else {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not found"}}"#
                        .to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let client = SorobanRpcClient::new(&url);
        let analyzer = StorageAnalyzer::new(client);

        let report = analyzer
            .inspect_contract_storage_keys(TEST_CONTRACT, &[account_key])
            .await
            .unwrap();

        assert_eq!(report.total_entries, 2);
        assert_eq!(report.instance_entries, 1);
        assert_eq!(report.other_entries, 1);

        // Account entry without liveUntilLedgerSeq has current_ttl = 0 and class = Other.
        // It must NOT drag down minimum_ttl to 0, nor be counted in average_ttl or expiring_soon.
        let summary = report
            .ttl_summary
            .expect("ttl_summary present for instance entry");
        assert_eq!(summary.minimum_ttl, 20000);
        assert_eq!(summary.maximum_ttl, 20000);
        assert_eq!(summary.average_ttl, 20000);
        assert_eq!(summary.expiring_entries_count, 0);
    }

    #[tokio::test]
    async fn test_all_other_entries_omits_ttl_summary() {
        let account_key = encode_ledger_key(&LedgerKey::Account(LedgerKeyAccount {
            account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([2u8; 32]))),
        }));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let ak = account_key.clone();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut sock) = conn else { break };
                let mut buf = [0u8; 16384];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.contains("getLatestLedger") {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","sequence":100}}"#.to_string()
                } else if req.contains("getLedgerEntries") {
                    // Only the account entry is returned by the RPC
                    format!(
                        r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[
                            {{"key":"{ak}","xdr":"AAAAAQAAAABpc25nAAAA","lastModifiedLedgerSeq":100}}
                        ],"latestLedger":100}}}}"#
                    )
                } else {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not found"}}"#
                        .to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let client = SorobanRpcClient::new(&url);
        let analyzer = StorageAnalyzer::new(client);

        let report = analyzer
            .inspect_contract_storage_keys(TEST_CONTRACT, &[account_key])
            .await
            .unwrap();

        assert_eq!(report.total_entries, 1);
        assert_eq!(report.other_entries, 1);
        assert_eq!(report.instance_entries, 0);
        assert!(report.ttl_summary.is_none());
    }
}
