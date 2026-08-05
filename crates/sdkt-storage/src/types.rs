use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StorageReport {
    pub contract_id: String,
    pub instance_entries: usize,
    pub persistent_entries: usize,
    pub temporary_entries: usize,
    pub total_size_bytes: Option<usize>,
    pub ttl_summary: Option<TtlInfoSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TtlInfoSummary {
    pub minimum_ttl: u32,
    pub maximum_ttl: u32,
    pub average_ttl: u32,
    pub expiring_entries_count: usize,
    pub estimated_rent_cost: Option<u64>,
}
