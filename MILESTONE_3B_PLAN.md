# Milestone 3B — Implementation Plan

**Baseline:** v0.3.0-m3a (Phase 3A complete)  
**Target:** Wire `sdkt-rpc` logic into `sdkt-cli`, finalize `sdkt-core` config, and expose the `storage` and `inspect` subcommands to the user.  
**Status:** Design

---

## 1. Objectives

1. Complete the CLI integration for the RPC capabilities built in M3A.
2. Allow users to configure storage-related limits and thresholds via `.sdkt.toml`.
3. Add a unified format output logic (`OutputFormat`) across the entire workspace.
4. Implement integration tests to verify the CLI executes correct RPC logic and prints appropriately.

---

## 2. Scope — In Scope

| Feature | Description |
|---------|-------------|
| `StorageConfig` | Add storage-related settings to `DevKitConfig` in `sdkt-core` |
| `OutputFormat` refactor | Move `OutputFormat` to `sdkt-core` and implement `FromStr`, re-export for `sdkt-xdr` and `sdkt-cli` |
| `sdkt storage check` | Add the `storage check <contract-id>` subcommand to the CLI |
| `sdkt storage estimate` | Add the `storage estimate <wasm>` subcommand to the CLI (deferred from M3A) |
| `sdkt inspect` | Add the `inspect <contract-id>` subcommand to the CLI |
| CLI Async | Add `tokio` to `sdkt-cli` and run `main` as an async entrypoint |
| Tests | End-to-end integration tests for the CLI subcommands |

## 2. Scope — Out of Scope

| Feature | Reason |
|---------|--------|
| `sdkt audit` (static analysis) | Planned for a future milestone (Milestone 4). Requires AST parsing. |
| Nested storage key enumeration | Advanced contract inspection requires parsing arbitrary ScVal keys. Will remain empty/basic for now. |

---

## 3. Files That Will Change

### Modified Files

| File | Change |
|------|--------|
| `crates/sdkt-core/src/config.rs` | Add `StorageConfig` struct, integrate into `DevKitConfig` with `#[serde(default)]` |
| `crates/sdkt-core/src/lib.rs` | Export `OutputFormat`, `StorageConfig` |
| `crates/sdkt-xdr/src/lib.rs` | Remove `OutputFormat` definition, import from `sdkt-core` |
| `crates/sdkt-cli/Cargo.toml` | Add `tokio` dependency, update `sdkt-rpc` dep |
| `crates/sdkt-cli/src/main.rs` | Refactor main to `async`, add `Storage` and `Inspect` subcommands, wire to `sdkt-rpc` functions |
| `crates/sdkt-cli/tests/cli_tests.rs`| Add integration tests for new subcommands |
| `README.md` | Document `sdkt storage` and `sdkt inspect` usage |
| `CHANGELOG.md` | Document release 0.3.0 |

---

## 4. Public APIs 

### `sdkt-core`
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct StorageConfig {
    pub max_entries: usize,
    pub ttl_warning_days: u32,
}

pub struct DevKitConfig {
    pub network: NetworkConfig,
    pub decode: DecodeConfig,
    #[serde(default)]
    pub storage: StorageConfig,
}

pub enum OutputFormat { Json, Pretty }
impl std::str::FromStr for OutputFormat { ... }
```

### `sdkt-cli`
```rust
#[derive(Subcommand)]
enum Commands {
    Decode { ... },
    Storage {
        #[command(subcommand)]
        action: StorageAction,
    },
    Inspect {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: OutputFormat,
    },
}
```

---

## 5. Test Plan

1. **Unit Tests:**
   - `sdkt-core`: verify `DevKitConfig` parses old TOML files without a `[storage]` section correctly (fallback to default).
   - `sdkt-core`: verify `OutputFormat::from_str` handles "json", "pretty", and invalid inputs.
2. **Integration Tests (`sdkt-cli`):**
   - Execute `sdkt inspect <contract-id>` with mock responses (or against future network) and verify stdout format.
   - Execute `sdkt storage check <contract-id>` and verify TTL calculations print correctly.
3. **Compilation:**
   - Verify `cargo build`, `fmt`, and `clippy` pass workspace-wide.

---

## 6. Acceptance Criteria

- [ ] `OutputFormat` is successfully migrated to `sdkt-core` without breaking existing `sdkt decode` functionality.
- [ ] `sdkt-cli` `main()` is an `async fn` running on Tokio.
- [ ] Running `cargo run -- storage check <valid-id>` successfully queries the RPC (or throws a proper RpcError).
- [ ] Running `cargo run -- inspect <valid-id>` successfully returns WASM hash info.
- [ ] Old `.sdkt.toml` files lacking `[storage]` still parse correctly.