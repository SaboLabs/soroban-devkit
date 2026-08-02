# Soroban DevKit (`sdkt`)

**Unified developer toolkit for Stellar and Soroban blockchain development.**

`sdkt` (Soroban DevKit) is a single-binary CLI tool that augments `stellar-cli` by providing
XDR decoding, configuration management, and a framework for static analysis — all with a
zero-config workflow and a plugin system for extensibility.

---

## Table of Contents

- [Quick Start](#quick-start)
- [Features](#features)
- [Workspace Structure](#workspace-structure)
- [Building](#building)
- [Installation](#installation)
- [Usage](#usage)
- [Configuration](#configuration)
- [Development](#development)
- [Testing](#testing)
- [Roadmap](#roadmap)
- [License](#license)

---

## Quick Start

```bash
# Clone and build
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo build --release

# Decode an XDR payload
./target/release/sdkt decode "AAAABAAAAAE="
# Output: {"ScVal":{"I32":1}}

# Decode with explicit type
./target/release/sdkt decode --type scval "AAAABAAAAAE="

# Pretty-print (default)
./target/release/sdkt decode "AAAABAAAAAE=" --format pretty

# Read from file
./target/release/sdkt decode --file payload.b64
```

---

## Features

### Implemented (v0.1.0)

| Feature | Description |
|---------|-------------|
| **XDR Decoding** | Decode base64 or hex-encoded XDR payloads to JSON |
| **Auto-detection** | Automatically detects XDR type (ScVal, TransactionEnvelope, ContractEvent, etc.) |
| **Explicit type hints** | Force decode as a specific XDR type via `--type` |
| **Dual input formats** | Accepts inline argument or file input via `--file` |
| **JSON / Pretty output** | Compact JSON or pretty-printed output via `--format` |
| **Offline operation** | No RPC calls needed for decoding — works fully offline |
| **Configuration profiles** | TOML-based config with network and decode settings |
| **Default testnet config** | Pre-configured for Soroban testnet |

### Supported XDR Types

- `ScVal`
- `TransactionEnvelope`
- `TransactionResult`
- `TransactionMeta`
- `LedgerKey`
- `LedgerEntry`
- `ContractEvent`
- Auto-detection across all of the above

### Planned (Future Releases)

See the [Roadmap](#roadmap) section.

---

## Workspace Structure

```
soroban-devkit/
├── Cargo.toml               # Workspace root manifest
├── Cargo.lock               # Lockfile (pinned versions)
├── GAP_ANALYSIS.md          # Market gap analysis & rationale
├── README.md
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SECURITY.md
├── CODE_OF_CONDUCT.md
├── LICENSE
├── .gitignore
├── .sdkt.toml               # Optional user/project config (example)
└── crates/
    ├── sdkt-core/           # Configuration engine
    │   └── src/
    │       ├── lib.rs        # Pub re-exports
    │       └── config.rs     # DevKitConfig, NetworkConfig, DecodeConfig
    ├── sdkt-xdr/            # XDR decoding engine
    │   └── src/
    │       └── lib.rs        # decode(), decode_bytes(), OutputFormat
    └── sdkt-cli/            # CLI binary
        └── src/
            └── main.rs       # Clap-based CLI entrypoint
```

### Crate Dependencies

```toml
# sdkt-core
serde, toml

# sdkt-xdr
stellar-xdr (v28), base64, hex, thiserror, serde, serde_json
sdkt-core (path)

# sdkt-cli
clap (v4, derive), serde_json
sdkt-core, sdkt-xdr (path)
```

---

## Building

### Prerequisites

- **Rust toolchain** (stable, edition 2021)
- Ensure `cargo` is in your PATH:

```bash
rustc --version
cargo --version
```

### Build from source

```bash
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo build --release
```

The binary will be at `target/release/sdkt`.

### Build without release optimizations (development)

```bash
cargo build
```

---

## Installation

### From source (current)

```bash
cargo install --path .
```

### Manual install

```bash
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo build --release
sudo cp target/release/sdkt /usr/local/bin/sdkt
```

### Via Cargo (once published)

```bash
cargo install sdkt-cli
```

### Via Homebrew (planned)

```bash
brew tap naninu123/tap
brew install sdkt
```

---

## Usage

### Decode XDR

```bash
# Basic decode (auto-detect type, pretty output)
sdkt decode "AAAABAAAAAE="

# Compact JSON output
sdkt decode "AAAABAAAAAE=" --format json

# Specify XDR type explicitly
sdkt decode "AAAABAAAAAE=" --type scval
sdkt decode "AAAABAAAAAE=" --type transactionenvelope

# Read payload from file
sdkt decode --file payload.b64
sdkt decode -i payload.hex --type transactionresult
```

### Available type hints

| Type hint (case-insensitive) |
|------------------------------|
| `scval` |
| `transactionenvelope` |
| `transactionresult` |
| `transactionmeta` |
| `ledgerkey` |
| `ledgerentry` |
| `contractevent` |
| `auto` (default) |

### Configuration file

Create `.sdkt.toml` in your project root:

```toml
[network]
rpc_url = "https://soroban-testnet.stellar.org"
passphrase = "Test SDF Network ; September 2015"

[decode]
max_depth = 64
allow_fallback_hex = true
```

If no config file is found, defaults to Soroban testnet settings.

---

## Configuration

### Default configuration

| Field | Default |
|-------|---------|
| `network.rpc_url` | `https://soroban-testnet.stellar.org` |
| `network.passphrase` | `Test SDF Network ; September 2015` |
| `decode.max_depth` | `32` |
| `decode.allow_fallback_hex` | `true` |

### Load config programmatically

```rust
use sdkt_core::{DevKitConfig, NetworkConfig, DecodeConfig};

// Load from file (falls back to default if missing)
let config = DevKitConfig::from_file(".sdkt.toml").unwrap();

// Or parse from TOML string
let config = DevKitConfig::from_toml(
    r#"
    [network]
    rpc_url = "http://localhost:8000"
    passphrase = "Standalone Network"

    [decode]
    max_depth = 64
    allow_fallback_hex = false
    "#
).unwrap();
```

---

## Development

### Prerequisites

Install Rust via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### Project layout

This is a Cargo workspace with three crates:

| Crate | Path | Purpose |
|-------|------|---------|
| `sdkt-core` | `crates/sdkt-core` | Configuration engine (shared types) |
| `sdkt-xdr` | `crates/sdkt-xdr` | XDR decoding library |
| `sdkt-cli` | `crates/sdkt-cli` | CLI binary |

### Adding dependencies

All dependencies are managed centrally. For workspace crates, use path dependencies:

```toml
[dependencies]
sdkt-core = { path = "../sdkt-core" }
```

### Code style

```bash
cargo fmt
cargo clippy --workspace -- -D warnings
```

### Running tests

```bash
cargo test --workspace
```

---

## Testing

```bash
# Run all tests
cargo test --workspace

# Run specific crate tests
cargo test -p sdkt-xdr
cargo test -p sdkt-core

# Run with output
cargo test --workspace -- --nocapture
```

### Current test coverage

- `sdkt-core`: 2 unit tests (default config, TOML parsing)
- `sdkt-xdr`: 6 unit tests + 1 doc test (base64, hex, type hints, empty payload, format variants)
- `sdkt-cli`: No unit tests (CLI integration tested manually)

---

## Roadmap

### Phase 1 (v0.1.0 — Baseline Release) ✅

- [x] XDR decoding CLI (`sdkt decode`)
- [x] Auto-detection of XDR types
- [x] TOML configuration loading
- [x] Documentation, contributing guide, security policy
- [x] Baseline test suite (9 tests)

### Phase 2 (v0.2.0 — Lifecycle Tooling)

- [ ] `sdkt storage check <contract-id>` — TTL timeline and extension cost
- [ ] `sdkt storage estimate <wasm>` — Predict deployment storage fees
- [ ] `sdkt inspect <contract-id>` — View WASM custom sections + storage state
- [ ] Interactive CLI menu for contract function listing

### Phase 3 (v0.3.0 — Static Analysis)

- [ ] `sdkt audit` — AST-based security scanning (missing `require_auth`, move violations)
- [ ] Custom lint plugin system
- [ ] Integration with `cargo-hack` for cross-version compatibility testing

### Phase 4 (v0.4.0 — CI/CD Integration)

- [ ] GitHub Actions for automated decoding + audit in pipelines
- [ ] Docker image for containerized runs
- [ ] Pre-commit hooks

### Phase 5 (v0.5.0 — Ecosystem Expansion)

- [ ] Plugin architecture for third-party linters/decoders
- [ ] WASM-based plugin sandboxing
- [ ] VS Code extension for inline XDR preview

---

## License

Dual-licensed under [MIT](LICENSE) or [Apache-2.0](LICENSE), at your option.

See [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines and [SECURITY.md](SECURITY.md) for vulnerability reporting.
