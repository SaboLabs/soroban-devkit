use crate::client::SorobanRpcClient;
use crate::error::RpcError;
use serde::Serialize;

/// Storage TTL info for a contract.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TtlInfo {
    pub contract_id: String,
    pub entries: Vec<TtlEntry>,
}

/// A single storage entry with human-readable TTL information.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TtlEntry {
    pub key: String,
    pub current_ttl: u32,
    pub expiration_time: String,
    pub days_remaining: u32,
    pub extension_cost_stroops: u64,
}

/// Calculates extension cost using a simple placeholder formula
/// because the exact design logic is unspecified.
pub fn calculate_extension_cost(ledger_delta: u32) -> u64 {
    ledger_delta as u64 * 100
}

/// Fetches TTL info from the RPC node and transforms it into a `TtlInfo` representation.
pub async fn get_ttl_info(
    client: &SorobanRpcClient,
    contract_id: &str,
) -> Result<TtlInfo, RpcError> {
    let ledger_info = client.get_ledger().await?;
    let current_ledger = ledger_info.sequence;

    // Fetch storage entries.
    // Contract code requires querying specific keys to see their TTLs.
    // If the contract provides keys we would pass them here. For now we use the contract_id itself as a stub representation
    // of a query if keys were not provided.
    // The design doesn't specify how to fetch ALL keys, as Soroban RPC `getLedgerEntries` requires explicit keys.
    // Assuming the user is querying the contract instance storage itself.
    // For standard smart contracts, the contract's own `ContractData` or instance requires a known key.

    // We pass empty keys, or possibly one key representing the contract itself.
    // In practice, `get_contract_storage` takes `keys: &[String]`.
    let storage_resp = client.get_contract_storage(contract_id, &[]).await?;

    let mut entries = Vec::new();
    for entry in storage_resp.entries {
        let current_ttl = if let Some(live_until) = entry.live_until_ledger_seq {
            live_until.saturating_sub(current_ledger)
        } else {
            0
        };

        // Rough estimation: 1 ledger ≈ 5 seconds
        let days_remaining = (current_ttl * 5) / (24 * 3600);
        let extension_cost_stroops = calculate_extension_cost(current_ttl);

        entries.push(TtlEntry {
            key: entry.key,
            current_ttl,
            expiration_time: format!("~{} days", days_remaining),
            days_remaining,
            extension_cost_stroops,
        });
    }

    Ok(TtlInfo {
        contract_id: contract_id.to_string(),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_extension_cost() {
        const SINGLE_ENTRY_TTL_30: u32 = 30;
        const SINGLE_ENTRY_TTL_100: u32 = 100;

        assert_eq!(calculate_extension_cost(SINGLE_ENTRY_TTL_30), 3000);
        assert_eq!(calculate_extension_cost(SINGLE_ENTRY_TTL_100), 10000);
        assert_eq!(calculate_extension_cost(0), 0);
    }
}
