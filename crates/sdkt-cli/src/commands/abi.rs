//! Shared ABI resolution helpers used by the `call`, `tx simulate`, `events`,
//! and `storage` command handlers.
//!
//! Two sources are supported, and they are always **mutually exclusive**:
//!
//! - `--abi <wasm-path>` — read a local WASM file and parse its embedded
//!   [`ContractSpec`].
//! - `--abi-contract <contract-id>` — fetch the deployed contract's WASM from
//!   the network via RPC and parse its [`ContractSpec`].
//!
//! [`load_local_abi`] handles just the local-file path.
//! [`resolve_abi_spec`] handles both paths (plus the "neither" → `None` case)
//! and is the entry point for all four callers.

use sdkt_rpc::{inspect_contract, SorobanRpcClient};
use sdkt_rpc::wasm::get_wasm_bytecode;
use sdkt_wasm::spec::parse_contract_spec;
use sdkt_wasm::ContractSpec;

/// Load a [`ContractSpec`] from a local WASM file.
///
/// Reads the file at `wasm_path` and parses the embedded contract spec.
/// Returns a human-readable `String` error on failure.
pub fn load_local_abi(wasm_path: &str) -> Result<ContractSpec, String> {
    let wasm_bytes =
        std::fs::read(wasm_path).map_err(|e| format!("Failed to read WASM: {}", e))?;
    parse_contract_spec(&wasm_bytes).map_err(|e| format!("Failed to parse ABI: {}", e))
}

/// Resolve an optional [`ContractSpec`] from one of two mutually exclusive
/// sources.
///
/// | `abi`      | `abi_contract` | Result                                              |
/// |------------|----------------|-----------------------------------------------------|
/// | `Some(p)`  | `None`         | Parse spec from the local WASM file at `p`          |
/// | `None`     | `Some(id)`     | Fetch deployed WASM via RPC and parse its spec      |
/// | `None`     | `None`         | `Ok(None)` — no ABI requested                       |
/// | `Some(_)`  | `Some(_)`      | `Err(...)` — caller must have already checked this  |
///
/// Callers that want a hard error on the conflicting case should call
/// [`check_abi_mutual_exclusion`] before this function and handle the error
/// themselves (e.g., by printing and calling `process::exit(1)`), so the
/// error message and exit code remain under the caller's control.
pub async fn resolve_abi_spec(
    abi: Option<&String>,
    abi_contract: Option<&String>,
    client: &SorobanRpcClient,
) -> Result<Option<ContractSpec>, String> {
    match (abi, abi_contract) {
        (Some(wasm_path), None) => load_local_abi(wasm_path).map(Some),
        (None, Some(id)) => {
            // on-chain retrieval: inspect_contract → wasm hash, then
            // get_wasm_bytecode → raw bytes, then parse_contract_spec.
            let inspection = inspect_contract(client, id).await.map_err(|e| match e {
                sdkt_rpc::RpcError::ContractNotFound => format!("contract {} not found", id),
                other => format!("{}", other),
            })?;
            let deployed_bytes = get_wasm_bytecode(client, &inspection.wasm_hash)
                .await
                .map_err(|e| format!("could not fetch on-chain WASM for {}: {}", id, e))?;
            parse_contract_spec(&deployed_bytes)
                .map(Some)
                .map_err(|e| format!("failed to parse deployed ABI: {}", e))
        }
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err("specify only one of --abi or --abi-contract".into()),
    }
}

/// Check that `--abi` and `--abi-contract` are not both supplied.
///
/// Returns `Err` with a ready-to-print message when both are `Some`.
/// Callers are responsible for handling the error (e.g. `eprintln!` +
/// `process::exit(1)`).
pub fn check_abi_mutual_exclusion(
    abi: Option<&String>,
    abi_contract: Option<&String>,
) -> Result<(), String> {
    if abi.is_some() && abi_contract.is_some() {
        Err("specify only one of --abi or --abi-contract".into())
    } else {
        Ok(())
    }
}
