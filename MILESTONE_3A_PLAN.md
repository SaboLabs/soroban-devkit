# Milestone 3A — Implementation Plan

**Baseline:** v0.1.0 (Phase 1 complete)  
**Target:** Add `sdkt storage` and `sdkt inspect` subcommands for Sorobum contract lifecycle inspection via RPC.  
**Status:** Design — not yet implemented.

---

## 1. Objectives

Deliver two new CLI subcommands that interact with the Soroban RPC API:

1. **`sdkt storage check <contract-id>`** — Query remaining TTL for a contract or user line and return a human-readable timeline with extension cost estimates.
2. **`sdkt inspect <contract-id>`** — Read WASM custom sections and current storage keys from a contract, displaying an interactive menu of read/write functions.

---

## 2. Scope — In Scope

| Feature | Description |
|---------|-------------|
| `sdkt-rpc` crate | New crate for Soroban RPC client interactions (separation from sdkt-xdr decoder) |
| `StorageConfig` | New config struct in `sdkt-core` for storage-related settings |
| `sdkt storage check` | Subcommand: fetch TTL info from RPC → format as JSON/pretty |
| `sdkt storage estimate <wasm>` | Subcommand: predict deployment storage fees (may defer to M3B) |
| `sdkt inspect` | Subcommand: fetch contract WASM + storage keys → interactive display |
| Config integration | Wire up `.sdkt.toml` network settings in CLI |
| Tests | Unit tests for new rpc crate + integration tests for new CLI subcommands |
| Documentation | Update README, CHANGELOG, add subcommand docs |

## 2. Scope — Out of Scope

| Feature | Reason |
|---------|--------|
| `sdkt audit` (Gap C) — static security analysis | Requires `syn` AST integration; planned for Milestone 3B |
| `sdkt storage estimate` full implementation | Can defer to Milestone 3B if RPC complexity is high |
| Plugin system | Mentioned in GAP_ANALYSIS; out of scope |
| Network interaction beyond read-only RPC calls | No transactions/wallet signing in 3A |

---

## 3. Files That Will Change

### New Files

| File | Purpose |
|------|---------|
| `crates/sdkt-rpc/Cargo.toml` | RPC crate manifest |
| `crates/sdkt-rpc/src/lib.rs` | RPC client entrypoint |
| `crates/sdkt-rpc/src/client.rs` | HTTP + JSON-RPC client to Soroban RPC |
| `crates/sdkt-rpc/src/storage.rs` | Storage TTL query + estimation logic |
| `crates/sdkt-rpc/src/inspect.rs` | WASM inspection + storage key listing |
| `crates/sdkt-rpc/src/error.rs` | RPC-specific error types (or reuse DecodeError) |
| `crates/sdkt-rpc/tests/integration.rs` | Integration tests with mock RPC server |

### Modified Files

| File | Change |
|------|--------|
| `crates/sdkt-core/src/config.rs` | Add `StorageConfig` struct + integrate into `DevKitConfig` |
| `crates/sdkt-core/src/lib.rs` | Export new config types |
| `crates/sdkt-cli/src/main.rs` | Add `Storage` and `Inspect` subcommand variants |
| `Cargo.toml` (workspace root) | Add `sdkt-rpc` to workspace members |
| `README.md` | Add `sdkt storage` and `sdkt inspect` docs |
| `CHANGELOG.md` | Add 0.2.0 section |

---

## 4. New Modules — Detailed

### `sdkt-rpc` crate

#### `client.rs` — SorobanRpcClient
```rust
pub struct SorobanRpcClient {
    endpoint: String,
    http_client: reqwest::Client,
}

impl SorobanRpcClient {
    pub fn new(endpoint: &str) -> Self;
    pub fn from_config(config: &NetworkConfig) -> Self;
    pub async fn get_health(&self) -> Result<HealthCheck, RpcError>;
    pub async fn get_ledger(&self) -> Result<LedgerInfo, RpcError>;
    pub async fn get_contract_storage(
        &self, contract_id: &str, keys: Vec<String>
    ) -> Result<StorageResponse, RpcError>;
}
```

#### `storage.rs` — Storage inspection
```rust
pub fn get_ttl_info(
    client: &SorobanRpcClient, contract_id: &str
) -> Result<TtlInfo, RpcError>;

pub struct TtlInfo {
    pub contract_id: String,
    pub entries: Vec<TtlEntry>,
}

pub struct TtlEntry {
    pub key: String,
    pub current_ttl: u32,
    pub expiration_time: String,
    pub days_remaining: u32,
    pub extension_cost_stroops: u64,
}
```

#### `inspect.rs` — Contract inspection
```rust
pub async fn inspect_contract(
    client: &SorobanRpcClient, contract_id: &str
) -> Result<ContractInspection, RpcError>;

pub struct ContractInspection {
    pub contract_id: String,
    pub wasm_hash: String,
    pub storage_keys: Vec<StorageKeyInfo>,
}

pub struct StorageKeyInfo {
    pub key: String,
    pub key_type: String,
    pub permissions: String,
}
```

#### `error.rs` — RPC errors
Leverage `thiserror` for structured errors:
```rust
pub enum RpcError {
    Http(reqwest::Error),
    Json(serde_json::Error),
    Rpc(String), // JSON-RPC error message
    ContractNotFound,
}
```

### `sdkt-core` — Config additions

```rust
// New section in DevKitConfig
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct StorageConfig {
    /// Max entries to fetch per page
    pub max_entries: usize,
    /// TTL warning threshold in days
    pub ttl_warning_days: u32,
}

// Add to DevKitConfig
pub struct DevKitConfig {
    pub network: NetworkConfig,
    pub decode: DecodeConfig,
    #[serde(default)]
    pub storage: StorageConfig,  // NEW — serde(default) so old TOML files still parse
}
```

### `sdkt-cli` — New subcommands

```rust
#[derive(Subcommand)]
enum Commands {
    Decode { ... },
    /// Inspect storage TTL for a contract
    Storage {
        #[command(subcommand)]
        action: StorageAction,
    },
    /// Inspect a contract's ABI and storage
    Inspect {
        /// Contract ID or hash
        contract_id: String,
        /// Output format
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
}

#[derive(Subcommand)]
enum StorageAction {
    /// Check remaining TTL for contract state
    Check {
        /// Contract ID
        contract_id: String,
        /// Output format
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Estimate deployment storage fees
    Estimate {
        /// Path to WASM file
        wasm: String,
    },
}
```

---

### DESIGN REVIEW IMPROVEMENTS (applied)

1. **OutputFormat migration**: `OutputFormat` (Json/Pretty) will be moved to `sdkt-core` with a re-export from `sdkt-xdr`, so all crates share one canonical type. This prevents duplication and keeps API consistent.

2. **StorageConfig Default impl**: Add `#[derive(Default)]` or manual `Default` impl for `StorageConfig` so `#[serde(default)]` works correctly.

3. **Clippy-safe format handling**: Instead of `format: String` + `parse_format()`, use `OutputFormat` enum directly in CLI struct via a `FromStr` impl. This enforces valid values at parse time and aligns with existing architecture.

4. **Explicit dependency**: Add `reqwest = { version = "0.22", features = ["json"] }` to `sdkt-rpc/Cargo.toml`.

5. **Async runtime**: Add `tokio = { version = "1", features = ["rt-multi-thread", "macros"] }` to `sdkt-cli` explicitly — do not rely on transitive features.

6. **Ownership clarity**: `get_contract_storage` will use `&[String]` instead of `Vec<String>` for keys to avoid unnecessary ownership transfer.

7. **RpcError full derives**: All `RpcError` variants get `#[from]` where applicable (Http, Json) to ensure ergonomic error composition via `?`.

8. **Test vectors for cost estimation**: Add `const` baseline test vectors for `calculate_extension_cost()` covering: single entry TTL=30, single entry TTL=100, batch cost calculation.

---

## 5. Public APIs (Revised)

### OutputFormat migration
```rust
// Moved to sdkt-core, re-exported from sdkt-xdr
pub enum OutputFormat { Json, Pretty }

impl FromStr for OutputFormat { ... }  // "json" → Json, "pretty" → Pretty
```

---

## 6. Internal APIs

| Module | Function | Purpose |
|--------|----------|---------|
| `sdkt-rpc::client` | `rpc_call(method, params)` | Generic JSON-RPC request |
| `sdkt-rpc::client` | `retry_request(...)` | HTTP retry with backoff |
| `sdkt-rpc::storage` | `parse_ledger_entry(entry)` | Parse raw RPC response → structured TTL |
| `sdkt-rpc::storage` | `calculate_extension_cost(ttl)` | Stroops cost for TTL extension |
| `sdkt-rpc::inspect` | `fetch_wasm(client, hash)` | Download contract WASM |
| `sdkt-rpc::inspect` | `list_storage_keys(client, contract_id)` | Enumerate storage keys |
| `sdkt-cli` | `print_storage_info(info, fmt)` | Format TTL info for CLI output |
| `sdkt-cli` | `print_inspection(info, fmt)` | Format inspection result for CLI output |

---

## 7. Test Strategy

### Unit Tests (per crate)

| Crate | Target Coverage |
|-------|----------------|
| `sdkt-rpc` | 8 tests — client construction, response parsing, TTL calculation, cost estimation, error paths |
| `sdkt-core` | 3 tests — new StorageConfig default, from_toml, from_file |
| `sdkt-cli` | 2 tests — subcommand parsing for storage/inspect |

### Integration Tests

| Test | Description |
|------|-------------|
| `test_storage_check_output` | Verify TTL query produces expected JSON/pretty format |
| `test_storage_check_contract_not_found` | Error handling when contract ID is invalid |
| `test_inspect_returns_keys` | Verify storage keys are listed correctly |
| `test_inspect_wasm_hash` | Verify WASM hash extraction |
| `test_config_network_integration` | Verify CLI uses `.sdkt.toml` network settings |

### Mock RPC server
- Use a minimal mock JSON-RPC server (e.g., `mockito` or inline `hyper` server) for integration tests to avoid live RPC dependency.

---

## 8. Documentation Updates

### README.md
- New sections: `sdkt storage`, `sdkt inspect`, updated workspace structure table, new crate in dependency graph

### CHANGELOG.md
```markdown
## [0.2.0] - 2026-08-15
### Added
- Storage TTL inspection (`sdkt storage check`)
- Contract inspection (`sdkt inspect`)
- Soroban RPC client crate (sdkt-rpc)
- Network config integration in CLI
- StorageConfig in sdkt-core
```

---

## 9. Backward Compatibility

| Area | Compatibility | Strategy |
|------|--------------|----------|
| Existing `sdkt decode` | ✅ Full | No changes to `decode` subcommand |
| `sdkt-core::DevKitConfig` | ⚠️ Partial | New `storage` field has `#[serde(default)]` to avoid breaking TOML parsing |
| CLI interface | ✅ Full | New subcommands don't conflict; existing `decode` unchanged |
| `.sdkt.toml` | ✅ Full | Storage config is optional, defaults applied if absent |

---

## 10. Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| **RPC protocol changes** | Medium | Use structured JSON-RPC parsing; isolate protocol changes in `client.rs` |
| **Network latency** | Low | Async client; show loading spinner |
| **WASM parsing complexity** | Medium | Focus on standard interface detection; fallback to raw hex output |
| **Stroop cost estimation accuracy** | Medium | Include disclaimer: estimates only; verify on-chain before acting |
| **Async integration** | Medium | Explicit `tokio = { version = "1", features = ["rt-multi-thread", "macros"] }` in `sdkt-cli`; do not rely on transitive features |
| **OutputFormat API drift** | Medium | Moved to `sdkt-core` to prevent drift between crates; `sdkt-xdr` re-exports for backward compat |

---

## 11. Estimated Implementation Order

| Step | Task | Crate | Est. Time |
|------|------|-------|-----------|
| 0 | Move `OutputFormat` to `sdkt-core`, add `Default` + `FromStr` | `sdkt-core` | 20 min |
| 1 | `StorageConfig` + `Default` impl + `#[serde(default)]` | `sdkt-core` | 20 min |
| 2 | `SorobanRpcClient` + `RpcError` + `client.rs` + Cargo.toml deps | `sdkt-rpc` | 60 min |
| 3 | `storage.rs` — TTL info + estimation | `sdkt-rpc` | 75 min |
| 4 | `inspect.rs` — contract inspection | `sdkt-rpc` | 60 min |
| 5 | CLI subcommands: `storage`, `inspect` | `sdkt-cli` | 45 min |
| 6 | Wire config + async runtime in CLI | `sdkt-cli` | 30 min |
| 7 | Unit tests (with const vectors) | all | 60 min |
| 8 | Integration tests with mock RPC | `sdkt-rpc` | 60 min |
| 9 | Documentation updates | root | 45 min |
| 10 | Final fmt + clippy + test | workspace | 15 min |

**Total estimated**: ~7.5 hours

---

## 12. Phase C — Design Validation (Updated)

### Possible Regressions
- Adding `storage` field to `DevKitConfig` could break old `.sdkt.toml` files — **Mitigated** by `#[serde(default)]` on the new field, with default `StorageConfig::default()`.

### Duplicate Code
- New RPC error types may overlap with `DecodeError` — **Mitigation**: Keep `RpcError` separate; `sdkt-xdr` errors remain XDR-specific.

### Ownership/lifetime Issues
- ✅ **Resolved**: `get_contract_storage` uses `&[String]` to avoid unnecessary ownership transfer
- ✅ **Resolved**: `SorobanRpcClient` uses `&self` (shared client, no `&mut` required)

### API Consistency (Revised)
- ✅ **Resolved**: `OutputFormat` moved to `sdkt-core` — single canonical type shared by `sdkt-xdr`, `sdkt-rpc`, and `sdkt-cli`
- ✅ **Resolved**: All subcommands now use `OutputFormat` enum directly (via `FromStr`), not `String` + manual parse

### Phase C — Final Validation

| Check | Status |
|-------|--------|
| API consistency | ✅ `OutputFormat` canonical in core, re-exported from xdr |
| Error handling | ✅ `RpcError` derives `thiserror` with `#[from]` on variants |
| Ownership/lifetime | ✅ No `&mut self` needed; slice refs for collections |
| Async boundaries | ✅ Explicit `tokio rt-multi-thread` in `sdkt-cli`; async in `sdkt-rpc` |
| Dependency minimization | ✅ `reqwest` only in `sdkt-rpc` (HTTP), `serde_json` only where used |
| Public API stability | ✅ No `#[non_exhaustive]` needed — new enums are additive |
| Testability | ✅ Const test vectors for cost calc; mock RPC server for integration |
| Future extensibility | ✅ Separate crates allow independent versioning |
| Documentation completeness | ✅ README updated in plan; CHANGELOG 0.2.0 section |
| Security considerations | ✅ Read-only RPC calls only; no wallet/signing in scope |

**Design validated after improvements. No major issues remain.**

### Design Review Findings Applied
1. ✅ `OutputFormat` moved to `sdkt-core` — prevents API drift
2. ✅ `StorageConfig` gets explicit `Default` impl
3. ✅ `OutputFormat::from_str` eliminates clippy string-match warnings
4. ✅ `reqwest` explicitly declared in `sdkt-rpc`
5. ✅ `tokio` features explicitly declared in `sdkt-cli`
6. ✅ `&[String]` (slice reference) instead of `Vec<String>` ownership — more flexible API
7. ✅ All `RpcError` variants get `#[from]` compositions
8. ✅ `const` test vectors for `calculate_extension_cost`

**Design validated. No major issues remain. Milestone 3A approved for implementation.**
