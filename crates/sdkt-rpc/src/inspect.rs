use crate::client::SorobanRpcClient;
use crate::error::RpcError;
use crate::wasm::get_wasm_bytecode;
use sdkt_wasm::{parse_contract_spec, ContractSpec};
use sdkt_xdr::{encode_ledger_key, extract_wasm_hash, LedgerKeyParams};
use serde::Deserialize;
use serde::Serialize;

/// A serializable projection of a contract's parsed ABI (from `contractspecv0`).
///
/// This reuses the existing `sdkt_wasm::parse_contract_spec` parser — it does NOT
/// introduce a new ABI decoder. It only lists the declared symbol names so the
/// on-chain inspection report can summarize a deployed contract's interface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ContractAbiSummary {
    pub functions: Vec<String>,
    pub events: Vec<String>,
    pub types: Vec<String>,
}

impl ContractAbiSummary {
    /// Project a parsed [`ContractSpec`] into the name-only summary.
    pub fn from_spec(spec: &ContractSpec) -> Self {
        Self {
            functions: spec.functions.iter().map(|f| f.name.clone()).collect(),
            events: spec.events.iter().map(|e| e.name.clone()).collect(),
            types: spec.custom_types.iter().map(|t| t.name.clone()).collect(),
        }
    }
}

/// The result of a contract inspection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractInspection {
    pub contract_id: String,
    pub wasm_hash: String,
    pub wasm_size: Option<usize>,
    /// Parsed on-chain ABI summary (functions / events / types). `None` when the
    /// on-chain WASM code cannot be fetched or has no `contractspecv0` section.
    pub abi: Option<ContractAbiSummary>,
    pub storage_summary: StorageSummary,
    pub ttl_info: Option<TtlInfoSummary>,
    pub storage_keys: Vec<StorageKeyInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TtlInfoSummary {
    pub minimum_ttl: u32,
    pub maximum_ttl: u32,
    pub average_ttl: u32,
    pub expiring_entries_count: usize,
    pub estimated_rent_cost: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StorageSummary {
    pub instance_entries: usize,
    pub persistent_entries: usize,
    pub temporary_entries: usize,
}

/// Metadata about a storage key discovered in the contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageKeyInfo {
    pub key: String,
    pub key_type: String,
    pub permissions: String,
}

/// Inspects a deployed contract to extract its WASM hash, on-chain WASM size,
/// and parsed ABI — reusing the existing `get_wasm_bytecode` + `parse_contract_spec`
/// primitives. Storage/TTL/storage-key fields are left for the caller's layer
/// (which has access to `StorageAnalyzer`) to populate, matching the existing
/// architecture where storage analysis lives outside `sdkt-rpc`.
///
/// Failures to fetch/parse the on-chain WASM code degrade gracefully: the
/// inspection still returns the `contract_id` + `wasm_hash` it already recovered,
/// with `wasm_size`/`abi` left as `None`. Only the initial contract-data lookup
/// failing (contract not on chain) is fatal.
pub async fn inspect_contract(
    client: &SorobanRpcClient,
    contract_id: &str,
) -> Result<ContractInspection, RpcError> {
    let encoded_key = encode_ledger_key(&LedgerKeyParams::ContractData(contract_id.to_string()))
        .map_err(|e| RpcError::Rpc(format!("Failed to encode ledger key: {e}")))?;

    let response = client
        .get_contract_storage(contract_id, &[encoded_key])
        .await?;

    if response.entries.is_empty() {
        return Err(RpcError::ContractNotFound);
    }

    let first_entry = &response.entries[0];

    let wasm_hash = extract_wasm_hash(&first_entry.xdr)
        .map_err(|e| RpcError::Rpc(format!("Failed to extract WASM hash: {e}")))?;

    // Enrich with on-chain WASM size + parsed ABI. Both steps are best-effort:
    // if the code entry is missing or unparseable, we keep what we have instead
    // of failing the whole inspection.
    let mut wasm_size = None;
    let mut abi = None;
    if let Ok(bytes) = get_wasm_bytecode(client, &wasm_hash).await {
        wasm_size = Some(bytes.len());
        if let Ok(spec) = parse_contract_spec(&bytes) {
            abi = Some(ContractAbiSummary::from_spec(&spec));
        }
    }

    Ok(ContractInspection {
        contract_id: contract_id.to_string(),
        wasm_hash,
        wasm_size,
        abi,
        storage_summary: StorageSummary::default(),
        ttl_info: None,
        storage_keys: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_summary_from_spec_lists_names() {
        // A spec with one function, one event, one type.
        let spec = ContractSpec {
            env_meta: None,
            functions: vec![sdkt_wasm::ContractFunction {
                name: "transfer".into(),
                doc: String::new(),
                parameters: vec![],
                outputs: vec![],
            }],
            custom_types: vec![sdkt_wasm::ContractType {
                name: "Asset".into(),
                kind: "Struct".into(),
                doc: String::new(),
                members: vec![],
            }],
            events: vec![sdkt_wasm::ContractEvent {
                name: "transfer".into(),
                doc: String::new(),
            }],
        };
        let summary = ContractAbiSummary::from_spec(&spec);
        assert_eq!(summary.functions, vec!["transfer".to_string()]);
        assert_eq!(summary.events, vec!["transfer".to_string()]);
        assert_eq!(summary.types, vec!["Asset".to_string()]);
    }

    #[test]
    fn abi_summary_default_is_empty() {
        let s = ContractAbiSummary::default();
        assert!(s.functions.is_empty());
        assert!(s.events.is_empty());
        assert!(s.types.is_empty());
    }
}
