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
        if contract_id.is_empty() {
            return Err(StorageError::InvalidContractId(
                "Contract ID cannot be empty".to_string(),
            ));
        }

        let ttl_info = sdkt_rpc::get_ttl_info(&self.client, contract_id).await?;

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

        let mut instance_entries = 0;
        let mut persistent_entries = 0;
        let mut temporary_entries = 0;
        let mut other_entries = 0;
        let mut detailed: Vec<StorageEntry> = Vec::with_capacity(ttl_info.entries.len());

        for entry in &ttl_info.entries {
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

            let class = classify_key(&entry.key);
            match class {
                StorageClass::Instance => instance_entries += 1,
                StorageClass::Persistent => persistent_entries += 1,
                StorageClass::Temporary => temporary_entries += 1,
                StorageClass::Other => other_entries += 1,
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
        let average_ttl = if count > 0 {
            (total_ttl / count as u64) as u32
        } else {
            0
        };
        if min_ttl == u32::MAX {
            min_ttl = 0;
        }

        let ttl_summary = Some(TtlInfoSummary {
            minimum_ttl: min_ttl,
            maximum_ttl: max_ttl,
            average_ttl,
            expiring_entries_count: expiring_soon,
            estimated_rent_cost: Some(total_cost),
        });

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
    use stellar_xdr::{
        ContractDataDurability, LedgerKey, LedgerKeyContractData, ScAddress, ScVal, WriteXdr,
    };

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
}
