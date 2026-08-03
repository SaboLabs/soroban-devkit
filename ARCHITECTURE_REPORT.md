# Soroban DevKit — Architecture Report

**As of:** 2026-08-02  
**Baseline:** v0.1.0  
**Prepared by:** IronClaw Agent

---

## 1. Workspace Overview

### Layout

The project is a Cargo virtual workspace with three crates:

```
soroban-devkit/
├── Cargo.toml              # Workspace root (no package, resolver = "2")
├── Cargo.lock
├── GAP_ANALYSIS.md
├── README.md
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SECURITY.md
├── CODE_OF_CONDUCT.md
├── LICENSE
├── .gitignore
├── .sdkt.toml              # Optional default user config
└── crates/
    ├── sdkt-core/          # Configuration engine
    ├── sdkt-xdr/           # XDR decoding engine
    └── sdkt-cli/           # CLI binary (entry point)
```

### Crate Map

| Crate | Purpose | Public API | Dependencies |
|-------|---------|------------|--------------|
| `sdkt-core` | Config parsing + network/decode settings | `DevKitConfig`, `NetworkConfig`, `DecodeConfig` | `serde`, `toml` |
| `sdkt-xdr` | Base64/hex → JSON XDR decoding | `decode()`, `decode_bytes()`, `decode_single()`, `auto_detect()`, `detect_and_decode()`, `format_json()`, `DecodeError`, `OutputFormat` | `stellar-xdr`, `base64`, `hex`, `thiserror`, `serde_json`, `sdkt-core` (path dep) |
| `sdkt-cli` | Clap-based CLI binary | `main()` entrypoint | `clap` (derive), `sdkt-core` (path), `sdkt-xdr` (path), `serde_json` |

---

## 2. Module Responsibilities

### sdkt-core (`crates/sdkt-core/src/`)

#### `lib.rs`
- Re-exports `config` module and `DevKitConfig`.

#### `config.rs`  
- **Responsibility**: Workspace-wide configuration structures and TOML file parsing.
- **Structures**: `DevKitConfig`, `NetworkConfig`, `DecodeConfig`
- **Key methods**:
  - `DevKitConfig::default()` — returns testnet defaults
  - `DevKitConfig::from_toml(&str)` — parse TOML string
  - `DevKitConfig::from_file<P: AsRef<Path>>(P)` — load from file, fallback to default

### sdkt-xdr (`crates/sdkt-xdr/src/`)

#### `lib.rs` (entire file is the module)
- **Responsibility**: XDR decode engine. Base64/hex string + raw bytes → JSON.
- **Pipeline**:
  1. `detect_and_decode(payload: &str) -> Vec<u8>` — tries base64 first, then hex
  2. `decode_bytes(raw: &[u8], type_hint: Option<&str>) -> serde_json::Value` — dispatches to typed decoder or auto-detect
  3. `decode_single<T: ReadXdr + Serialize>()` — generic XDR reader
  4. `auto_detect()` — tries `ScVal`, `TransactionEnvelope`, `ContractEvent` in order
  5. `format_json()` — serialize to compact JSON or pretty JSON

- **Supported XDR types** (via `stellar-xdr` crate v28):
  - `ScVal`, `TransactionEnvelope`, `TransactionResult`
  - `TransactionMeta`, `LedgerKey`, `LedgerEntry`, `ContractEvent`

- **Public types**: `DecodeError` (enum, thiserror), `OutputFormat` (enum, Default = Pretty)

#### Test Coverage
6 unit tests + 1 doc test:
- `test_invalid_base64`
- `test_valid_scval_integer_base64`
- `test_auto_decode_scval`
- `test_empty_payload`
- `test_unknown_type`
- `test_json_vs_pretty`

### sdkt-cli (`crates/sdkt-cli/src/`)

#### `main.rs` (77 lines)
- **Responsibility**: CLI entrypoint. Parses arguments via Clap derive.
- **Current subcommand**: `decode`
- **Arguments**: `payload` (positional), `--type`, `--format`, `--file`
- **Flow**: Read input (from arg or file) → call `sdkt_xdr::decode()` → println

---

## 3. Public API Map

```
sdkt-core
├── DevKitConfig
│   ├── network: NetworkConfig
│   │   ├── rpc_url: String
│   │   └── passphrase: String
│   └── decode: DecodeConfig
│       ├── max_depth: usize (default 32)
│       └── allow_fallback_hex: bool (default true)
├── NetworkConfig
├── DecodeConfig
└── from_toml(), from_file(), default()

sdkt-xdr
├── decode(payload, type_hint, format) -> Result<String, DecodeError>
├── decode_bytes(raw, type_hint) -> Result<Value, DecodeError>
├── format_json(value, format) -> Result<String, DecodeError>
├── OutputFormat { Json, Pretty }
└── DecodeError {
    Base64(DecodeError),
    Hex(FromHexError),
    XdrParse(String, stellar_xdr::Error),
    TypeUnknown(String),
    EmptyPayload,
    Json(serde_json::Error)
}

sdkt-cli
└── main() — CLI with `decode` subcommand
```

---

## 4. Dependency Graph

```
sdkt-cli
├── sdkt-core  (path: ../sdkt-core)
├── sdkt-xdr   (path: ../sdkt-xdr)
└── serde_json

sdkt-xdr
├── sdkt-core  (path: ../sdkt-core)
├── stellar-xdr v28
├── base64
├── hex
├── thiserror
├── serde
└── serde_json

sdkt-core
├── serde
└── toml
```

---

## 5. Current CLI Commands

| Command | Subcommand | Args | Status |
|---------|------------|------|--------|
| `sdkt` | `decode` | `payload`, `--type`, `--format`, `--file` | Implemented (v0.1.0) |

---

## 6. Decode Pipeline (Current)

```
Input (String or File)
   │
   ▼
detect_and_decode() ── tries base64 → hex fallback
   │
   ▼
raw bytes
   │
   ▼
decode_bytes() ── dispatch by type_hint or auto_detect()
   │            ├── scval       → decode_single::<ScVal>()
   │            ├── transactionenvelope → decode_single::<TransactionEnvelope>()
   │            └── auto → tries ScVal, TransactionEnvelope, ContractEvent
   │
   ▼
serde_json::Value
   │
   ▼
format_json() ── OutputFormat::Json or Pretty
   │
   ▼
println!
```

---

## 7. Extension Points

| Area | Current State | Future Impact |
|------|--------------|---------------|
| Config | `DevKitConfig` struct | Can add new config sections (e.g., `[storage]`, `[audit]`) |
| XDR decode | `decode_bytes()` match statement | Add new XDR type arms easily |
| CLI | Clap derive `enum Commands` | Add new variants easily |
| Types | `OutputFormat` enum | Currently in `sdkt-xdr`; **Milestone 3A will migrate to `sdkt-core`** with re-export from `sdkt-xdr` for backward compat |
| Error handling | `DecodeError` enum | Add new error variants via enum |
| RPC layer | **Does not exist** | **Milestone 3A will add `sdkt-rpc` crate** as abstraction boundary

---

## 8. Technical Debt

| Issue | Severity | Description |
|-------|----------|-------------|
| **No config integration** | Medium | `DevKitConfig` exists but `sdkt-cli` never loads it; config from `.sdkt.toml` is ignored at CLI level |
| **No network interaction** | Low | `sdkt-core::NetworkConfig::rpc_url` is never used — no RPC client in any crate |
| **Limited auto-detection** | Medium | `auto_detect()` only tries 3 types; `TransactionResult`, `TransactionMeta`, `LedgerKey`, `LedgerEntry` are unsupported in auto mode |
| **No error recovery** | Medium | If base64 decode fails, hex attempt isn't made — it returns immediately (due to `detect_and_decode` early-return on base64 failure) |
| **CLI format is a string** | Low | `format: String` should ideally be an enum to enforce valid values at parse time |
| **No logging** | Low | No `tracing` or `log` crate integrated — debugging requires manual instrumentation |
| **No subcommand traits/traits pattern** | Low | CLI is flat; adding complex subcommands will grow `main.rs` significantly |

---

## 9. Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| **stellar-xdr upgrade** | Medium | v28 pinned; upgrading to newer stellar-xdr versions could break API |
| **Config drift** | Medium | If `.sdkt.toml` isn't loaded by CLI, config values diverge from user intent |
| **Type explosion** | Low | Each new XDR type requires a match arm + decoder function — mechanical but error-prone |
| **Offline-only** | Low | Current `decode` works offline; adding `storage`/`inspect` requires network — must handle offline gracefully |

---

## 10. Key Observations for Milestone 3A

1. **Gap B (Storage)** and **Gap E (Inspect)** both require RPC interaction — `sdkt-xdr` crate would need to be split or a new `sdkt-rpc` crate created.
2. **GAP_ANALYSIS.md** explicitly lists planned commands: `sdkt storage` (check, estimate) and `sdkt inspect` (contract ID).
3. `NetworkConfig` already exists but is unused — this is the natural place to start for RPC interaction.
4. Current architecture cleanly separates config/core/xdr/cli — adding an `sdkt-rpc` crate follows the same pattern.

---

## Appendix: File Inventory

| File | Lines | Purpose |
|------|-------|---------|
| `/Cargo.toml` | 7 | Workspace root |
| `/crates/sdkt-core/Cargo.toml` | 8 | Core crate manifest |
| `/crates/sdkt-core/src/lib.rs` | 7 | Core re-exports |
| `/crates/sdkt-core/src/config.rs` | 100 | Config structures + tests |
| `/crates/sdkt-xdr/Cargo.toml` | 13 | XDR crate manifest |
| `/crates/sdkt-xdr/src/lib.rs` | 231 | XDR decoding engine + tests |
| `/crates/sdkt-cli/Cargo.toml` | 10 | CLI crate manifest |
| `/crates/sdkt-cli/src/main.rs` | 77 | CLI entrypoint |

**Total source lines (non-test)**: ~162  
**Total test lines**: ~74
