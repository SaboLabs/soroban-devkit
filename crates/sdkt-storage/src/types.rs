use serde::{Deserialize, Serialize};

/// Classification of a storage entry by its Soroban storage durability/type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StorageClass {
    /// Contract instance storage (the `ContractInstance` singleton).
    #[default]
    Instance,
    /// Persistent `ContractData` entries.
    Persistent,
    /// Temporary `ContractData` entries.
    Temporary,
    /// Anything that is not a Soroban contract storage key (e.g. account, code).
    Other,
}

impl StorageClass {
    /// Human-readable singular label used in CLI pretty output.
    pub fn label(&self) -> &'static str {
        match self {
            StorageClass::Instance => "instance",
            StorageClass::Persistent => "persistent",
            StorageClass::Temporary => "temporary",
            StorageClass::Other => "other",
        }
    }
}

/// A single classified storage entry produced by the analyzer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageEntry {
    /// The base64 XDR `LedgerKey` as returned by RPC.
    pub key: String,
    /// Classified storage type.
    pub class: StorageClass,
    pub current_ttl: u32,
    pub days_remaining: u32,
    pub extension_cost_stroops: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StorageReport {
    pub contract_id: String,
    /// Total number of storage entries observed.
    pub total_entries: usize,
    pub instance_entries: usize,
    pub persistent_entries: usize,
    pub temporary_entries: usize,
    pub other_entries: usize,
    pub total_size_bytes: Option<usize>,
    pub ttl_summary: Option<TtlInfoSummary>,
    /// Per-entry detail (additive; absent in legacy serialized reports).
    #[serde(default)]
    pub entries: Vec<StorageEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TtlInfoSummary {
    pub minimum_ttl: u32,
    pub maximum_ttl: u32,
    pub average_ttl: u32,
    pub expiring_entries_count: usize,
    pub estimated_rent_cost: Option<u64>,
}
