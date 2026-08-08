//! WASM metadata retrieval from Soroban RPC.
//!
//! Fetches a contract's WASM bytecode via `getLedgerEntries` using the WASM hash,
//! then delegates parsing to `sdkt-wasm` to extract metadata.

use crate::{RpcError, SorobanRpcClient};
use sdkt_wasm::WasmMetadata;
use sdkt_xdr::{encode_ledger_key, extract_wasm_bytecode, LedgerKeyParams};

/// Fetch WASM bytecode by hash from the RPC node and parse its metadata.
///
/// The `wasm_hash` should be a hex-encoded 32-byte hash obtained from
/// a prior contract inspection (e.g. via `inspect_contract`).
pub async fn get_wasm_metadata(
    client: &SorobanRpcClient,
    wasm_hash: &str,
) -> Result<WasmMetadata, RpcError> {
    let encoded_key = encode_ledger_key(&LedgerKeyParams::ContractCode(wasm_hash.to_string()))
        .map_err(|e| RpcError::Rpc(format!("Failed to encode WASM ledger key: {e}")))?;

    let response = client.get_contract_storage("", &[encoded_key]).await?;

    if response.entries.is_empty() {
        return Err(RpcError::Rpc("WASM code not found on network".to_string()));
    }

    let wasm_bytes = extract_wasm_bytecode(&response.entries[0].xdr)
        .map_err(|e| RpcError::Rpc(format!("Failed to extract WASM bytecode: {e}")))?;

    let metadata = sdkt_wasm::parse_metadata(&wasm_bytes)
        .map_err(|e| RpcError::Rpc(format!("Failed to parse WASM metadata: {e}")))?;

    Ok(metadata)
}

/// Fetch only the raw WASM bytecode for a contract's code entry, identified by
/// its WASM hash. Reuses the same `getLedgerEntries` + `extract_wasm_bytecode`
/// path as [`get_wasm_metadata`] but returns the raw bytes so callers can run
/// additional offline parsers (e.g. `sdkt_wasm::parse_contract_spec`) without a
/// second network round-trip for metadata.
pub async fn get_wasm_bytecode(
    client: &SorobanRpcClient,
    wasm_hash: &str,
) -> Result<Vec<u8>, RpcError> {
    let encoded_key = encode_ledger_key(&LedgerKeyParams::ContractCode(wasm_hash.to_string()))
        .map_err(|e| RpcError::Rpc(format!("Failed to encode WASM ledger key: {e}")))?;

    let response = client.get_contract_storage("", &[encoded_key]).await?;

    if response.entries.is_empty() {
        return Err(RpcError::Rpc("WASM code not found on network".to_string()));
    }

    extract_wasm_bytecode(&response.entries[0].xdr)
        .map_err(|e| RpcError::Rpc(format!("Failed to extract WASM bytecode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkt_xdr::LedgerKeyParams;

    #[test]
    fn test_encode_contract_code_key() {
        let hash = "0000000000000000000000000000000000000000000000000000000000000001";
        let encoded = encode_ledger_key(&LedgerKeyParams::ContractCode(hash.to_string()));
        assert!(encoded.is_ok());
        let b64 = encoded.unwrap();
        assert!(!b64.is_empty());
    }

    #[test]
    fn test_encode_contract_code_key_invalid_length() {
        let short_hash = "0011";
        let result = encode_ledger_key(&LedgerKeyParams::ContractCode(short_hash.to_string()));
        assert!(result.is_err());
    }
}
