# Soroban DevKit — Architecture Report

**As of:** 2026-08-14
**Version:** v2.5.0
**Prepared by:** IronClaw Agent

---

## 1. Workspace Overview

### Layout

The project is a Cargo virtual workspace with 8 publishable crates plus 1 excluded browser-only crate:

```
soroban-devkit/
├── Cargo.toml              # Workspace root (resolver = "2")
├── Cargo.lock
├── README.md
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SECURITY.md
├── CODE_OF_CONDUCT.md
├── LICENSE
├── ROADMAP.md
├── RELEASE_READINESS.md
├── ARCHITECTURE_REPORT.md
├── crates/
│   ├── sdkt-core/          # Shared types, config, validation
│   ├── sdkt-xdr/           # XDR decode/encode
│   ├── sdkt-wasm/          # WASM parsing, ABI, offline diff
│   ├── sdkt-rpc/           # Soroban RPC client
│   ├── sdkt-storage/       # WASM cache, identity/keystore
│   ├── sdkt-audit/         # Static security analysis
│   ├── sdkt-audit-example-rule/  # Reference plugin
│   ├── sdkt-cli/           # CLI binary (entry point)
│   └── sdkt-playground/    # Browser-only glue (excluded from workspace)
├── docs/
├── website/
└── .github/
```

### Crate Map

| Crate | Role | Dependencies |
|-------|------|--------------|
| `sdkt-core` | Configuration, validation, shared types | None internal |
| `sdkt-xdr` | XDR decode/encode (ScVal, TransactionEnvelope, ContractEvent) | `sdkt-core`, `sdkt-wasm` |
| `sdkt-wasm` | WASM parsing, ContractSpec, offline diff, UpgradeVerdict | `sdkt-core` |
| `sdkt-rpc` | Soroban RPC client (reqwest) | `sdkt-core`, `sdkt-xdr`, `sdkt-wasm` |
| `sdkt-storage` | WASM cache, ED25519 identity/keystore, StorageAnalyzer | `sdkt-rpc`, `sdkt-wasm` |
| `sdkt-audit` | Static security analysis (AUTH-001/002/003, MOVE-001) | `sdkt-wasm` |
| `sdkt-audit-example-rule` | Reference plugin rule (loadable as `.so`/`.wasm`) | `sdkt-audit` |
| `sdkt-cli` | CLI binary (clap derive) | All 7 supporting crates |
| `sdkt-playground` | Browser-only wasm-bindgen wrapper (excluded from workspace) | `sdkt-wasm` |

---

## 2. Dependency Graph

```
sdkt-core  → (nothing internal)
sdkt-xdr   → sdkt-core, sdkt-wasm
sdkt-wasm  → sdkt-core
sdkt-rpc   → sdkt-core, sdkt-xdr, sdkt-wasm
sdkt-storage → sdkt-rpc, sdkt-wasm
sdkt-audit → sdkt-wasm
sdkt-audit-example-rule → sdkt-audit
sdkt-cli   → sdkt-core, sdkt-xdr, sdkt-wasm, sdkt-rpc, sdkt-storage, sdkt-audit, sdkt-audit-example-rule
sdkt-playground → sdkt-wasm (excluded from workspace)
```

---

## 3. Crate Responsibilities

### sdkt-core
- `DevKitConfig`, `NetworkConfig`, `DecodeConfig`, `ContractConfig`
- `OutputFormat`, `FeeConfig`, `FeeEstimator`
- `DependencyFetcher`, `GitFetcher`, `PathResolver` (M35.1)
- Package validation (`validate_manifest`, `validate_dependencies`)
- `resolve_deploy_order`, `validate_project` (topological sort)
- Network safety guards (`guard_mutating_network`, M39)

### sdkt-xdr
- Base64/hex → JSON XDR decoding
- `decode()`, `decode_bytes()`, `auto_detect()`
- Supported types: ScVal, TransactionEnvelope, TransactionResult, TransactionMeta, LedgerKey, LedgerEntry, ContractEvent
- `DecodeError` enum, `OutputFormat`

### sdkt-wasm
- WASM module parsing (metadata, exports, imports, custom sections)
- `parse_metadata()`, `parse_contract_spec()`
- `ContractSpec`, `FunctionSignature`, `CustomType`, `EventSpec`
- `SpecDiff`, `UpgradeVerdict` (M14 upgrade safety)
- `WasmError` enum

### sdkt-rpc
- `SorobanRpcClient` (persistent pooled reqwest)
- `inspect_contract`, `get_wasm_bytecode`
- Transaction simulate/submit, events, account, fee estimation
- `RpcError` enum

### sdkt-storage
- `WasmCache` (atomic tempfile rename, per-network isolation)
- `IdentityStore` (ED25519 keystore, key generate/import/list/show/delete/default)
- `StorageAnalyzer` (Instance/Persistent/Temporary classification)
- `CacheInfo`, `StorageError`

### sdkt-audit
- `scan_all_functions()`, `AuditContext`, `FnScan`
- `RuleRegistry`, `AuditRule` trait
- Built-in rules: AUTH-001/002/003, MOVE-001
- Plugin loaders: native (.so/.dylib/.dll) via libloading, WASM via extism
- `PluginStore` (M40 local offline store)

### sdkt-audit-example-rule
- Reference plugin crate (EXAMPLE-001)
- Loadable as native `.so` or `.wasm`

### sdkt-cli
- CLI entrypoint using clap derive
- Routes commands to internal crates
- Feature flags: `plugins`, `wasm-plugins`, `provenance`
- Error handling via `eprintln!` + non-zero exit

### sdkt-playground
- Browser-only wasm-bindgen wrapper
- Excluded from workspace (built separately for wasm32-unknown-unknown)

---

## 4. CLI Commands (v2.5.0)

| Command | Subcommand | Purpose | Network |
|---------|------------|---------|---------|
| `decode` | `<xdr>` | Base64/hex → JSON | No |
| `inspect` | `<contract-id>` | Contract ABI + storage | Yes |
| `storage` | `check/analyze/estimate` | Storage TTL analysis | Yes/No |
| `verify` | `--contract <id>` | On-chain vs local WASM hash | Yes |
| `health` | `--contract <id>` | Unified posture report | Yes |
| `tx` | `inspect/validate/simulate/sign/submit/build` | Transaction lifecycle | Mixed |
| `events` | `<contract-id>` | Event explorer | Yes |
| `account` | `<address>` | Balances + signers | Yes |
| `fee` | `estimate` | Fee estimation | Yes |
| `wasm` | `inspect/metadata/cache` | WASM management | Mixed |
| `diff` | `--old-wasm/--new-wasm` | Offline WASM diff | No |
| `audit` | `<path.rs>` | Static security analysis | No |
| `identity` | `generate/import/list/show/delete/default` | Keystore management | No |
| `network` | `add/list/show/remove` | Named profiles | No |
| `init` | `<name>` | Scaffold project | No |
| `deploy` | `--wasm/--salt` | Upload + instantiate | Yes |
| `build` | | Compile workspace contracts | No |
| `lock` | `generate/verify/show` | Lock file management | No |
| `package` | `validate/fetch/update/pack/publish` | Package manifests | Mixed |
| `project` | `deploy` | Multi-contract workspace deploy | Yes |
| `plugin` | `list/show/install/remove/update` | Local plugin store | No |
| `completions` | `<shell>` | Shell completion scripts | No |

---

## 5. Feature Flags

| Flag | Crate | Effect |
|------|-------|--------|
| `plugins` | sdkt-cli | Load native shared-library plugins |
| `wasm-plugins` | sdkt-cli | Load sandboxed WASM plugins |
| `provenance` | sdkt-cli | Append git commit + build date to version |
| `plugins` | sdkt-audit | Enable native plugin loader |
| `wasm-plugins` | sdkt-audit | Enable WASM plugin loader |

---

## 6. Testing Strategy

- Unit tests: `#[cfg(test)] mod tests` in each crate
- Integration tests: `crates/sdkt-cli/tests/` (assert_cmd)
- Fixtures: `crates/sdkt-cli/tests/fixtures/` (us_old.wasm, us_new.wasm)
- CI: `.github/workflows/ci.yml` (fmt, clippy, test on Linux/macOS/MSRV/Windows)
- Compatibility: `.github/workflows/compatibility.yml` (real stellar/soroban-examples)
- Release: `.github/workflows/release.yml` (cross-platform binaries + crates.io publish)

---

## 7. Extension Points

| Area | Current State |
|------|---------------|
| New XDR type | Add arm in `sdkt-xdr/src/lib.rs` `decode_bytes()` |
| New CLI command | Add variant in `sdkt-cli/src/main.rs` + handler function |
| New audit rule | Implement `AuditRule` trait + register in `RuleRegistry` |
| Plugin | Build `.so` or `.wasm` with C-ABI/wasm-bindgen export |
| New config section | Add struct in `sdkt-core/src/config.rs` |
| New network | Add named profile via `sdkt network add` |

---

## 8. Security

- Mainnet safety (M39): mutating commands refuse mainnet without explicit `--network-profile`/`--rpc-url`
- Plugin sandboxing: WASM plugins run in Extism sandbox
- Checksum verification: install.sh verifies SHA-256 before extraction
- Atomic writes: cache uses tempfile rename to prevent corruption
- Identity: ED25519 keys stored locally, never transmitted

---

## 9. Current Maturity

- 40+ milestones merged to `main`
- All 8 crates published to crates.io at v2.5.0
- GitHub release: macOS (x86_64, aarch64), Linux (x86_64)
- Web Playground deployed to GitHub Pages
- CI pipeline green on Linux/macOS/Windows

---

## 10. Known Gaps

- Windows binary not yet available (no release asset, no install docs)
- No Windows-specific path tests (cache namespace, identity keystore)
- Hosted package registry / remote plugin marketplace deferred
- CI does not cover sdkt-playground crate
