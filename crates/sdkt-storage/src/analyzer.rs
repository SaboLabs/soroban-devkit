use crate::error::StorageError;
use crate::types::{StorageReport, TtlInfoSummary};
use sdkt_rpc::SorobanRpcClient;

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

        // We fetch the TTL info which queries the underlying get_contract_storage representation for the contract currently
        let ttl_info = sdkt_rpc::get_ttl_info(&self.client, contract_id).await?;

        if ttl_info.entries.is_empty() {
            return Ok(StorageReport {
                contract_id: contract_id.to_string(),
                ..Default::default()
            });
        }

        let mut min_ttl = u32::MAX;
        let mut max_ttl = 0;
        let mut total_ttl: u64 = 0;
        let mut expiring_soon = 0;
        let mut total_cost: u64 = 0;

        for entry in &ttl_info.entries {
            let ttl = entry.current_ttl;
            if ttl < min_ttl {
                min_ttl = ttl;
            }
            if ttl > max_ttl {
                max_ttl = ttl;
            }
            total_ttl += ttl as u64;

            if ttl < 17280 {
                // ~1 day (17280 ledgers @ 5s)
                expiring_soon += 1;
            }
            total_cost += entry.extension_cost_stroops;
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

        // For now, mapping everything as instance_entries for basic testing because
        // extracting strict StorageType (Instance/Persistent/Temporary) needs deeper XDR payload parsing,
        // which will be expanded in the next phases of Milestone 4.
        let report = StorageReport {
            contract_id: contract_id.to_string(),
            instance_entries: ttl_info.entries.len(),
            persistent_entries: 0,
            temporary_entries: 0,
            total_size_bytes: None,
            ttl_summary,
        };

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkt_rpc::SorobanRpcClient;

    #[tokio::test]
    async fn test_inspect_empty_contract_id() {
        let client = SorobanRpcClient::new("http://localhost");
        let analyzer = StorageAnalyzer::new(client);
        let err = analyzer.inspect_contract_storage("").await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidContractId(_)));
    }
}
