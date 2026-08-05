# Milestone 3A — Completion Report

## Summary
Milestone 3A successfully establishes the foundation for Soroban RPC interactions. The `sdkt-rpc` crate was created to isolate network logic, and essential JSON-RPC methods for ledger state, health, and contract storage were implemented. We also added TTL calculation and WASM hash extraction logic backed by real XDR decoding. 

The implementation focused purely on the RPC and core logic crates (`sdkt-rpc` and `sdkt-xdr`). The CLI integration (`sdkt-cli`) and `sdkt-core` config changes were deferred to Milestone 3B to keep the PR scope focused and testable.

## Features Implemented
- **`sdkt-rpc` crate:** New workspace member for Soroban RPC client interactions.
- **`SorobanRpcClient`:** Configurable reqwest-based JSON-RPC client.
- **RPC Methods:**
  - `get_health()`
  - `get_ledger()`
  - `get_contract_storage()`
- **Storage Inspection (`storage.rs`):**
  - Live TTL info retrieval via `get_ttl_info()`.
  - Extension cost estimation logic.
- **Contract Inspection (`inspect.rs`):**
  - WASM hash extraction from live ledger entries.
- **XDR Enhancements (`sdkt-xdr`):**
  - `encode_ledger_key()` for building `ContractData` keys.
  - `extract_wasm_hash()` for parsing WASM hashes out of decoded XDR.

## Public APIs Added
### `sdkt-rpc`
```rust
pub struct SorobanRpcClient { ... }
impl SorobanRpcClient {
    pub fn new(endpoint: &str) -> Self;
    pub fn from_config(config: &NetworkConfig) -> Self;
    pub async fn get_health(&self) -> Result<HealthCheck, RpcError>;
    pub async fn get_ledger(&self) -> Result<LedgerInfo, RpcError>;
    pub async fn get_contract_storage(
        &self, contract_id: &str, keys: &[String]
    ) -> Result<StorageResponse, RpcError>;
}

pub async fn get_ttl_info(client: &SorobanRpcClient, contract_id: &str) -> Result<TtlInfo, RpcError>;
pub fn calculate_extension_cost(days_remaining: u32, base_fee: u64) -> u64;
pub async fn inspect_contract(client: &SorobanRpcClient, contract_id: &str) -> Result<ContractInspection, RpcError>;
```

### `sdkt-xdr`
```rust
pub enum LedgerKeyParams { ContractData(String) }
pub fn encode_ledger_key(params: &LedgerKeyParams) -> Result<String, DecodeError>;
pub fn extract_wasm_hash(base64_ledger_entry: &str) -> Result<String, DecodeError>;
```

## Validation Results
- **Build:** Passed (`cargo build --workspace`)
- **Fmt:** Passed (`cargo fmt --all`)
- **Clippy:** Passed (`cargo clippy --workspace --all-targets -- -D warnings` - 0 warnings)
- **Tests:** Passed (23 passing tests across the workspace). New tests cover XDR encoding/decoding and basic structure validations.

## Git Tag
`v0.3.0-m3a`

## Known Limitations
- The HTTP/RPC client logic does not yet have integration tests against a mock server (e.g. `mockito`), so unit tests for live methods are currently structural.
- Full nested storage key enumeration in `inspect.rs` is not yet implemented (returns empty `Vec`).
- CLI integration is absent (subcommands not yet wired).

## Next Milestone (3B)
Milestone 3B will focus on completing the user-facing CLI integration for these RPC tools, wiring up the configuration layer in `sdkt-core`, and adding proper integration tests for the CLI output.