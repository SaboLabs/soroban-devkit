# Soroban DevKit (`sdkt`)

`sdkt` is a unified, offline-capable toolkit for Stellar / Soroban development. It unifies contract inspection, XDR decoding, storage TTL analysis, ABI-aware decoding, static security analysis, WASM diffing, and an upgrade-safety verdict into a single CLI — instead of juggling 5+ separate tools.

## Installation

```bash
# From crates.io (after the v1.0 release)
cargo install sdkt-cli

# Or build from source
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli
```

Requires Rust Edition 2021 (`rustup toolchain install stable`).

## Commands

| Command | Purpose |
|---------|---------|
| `sdkt decode <xdr>` | Decode base64 XDR (`--type ScVal|TransactionEnvelope|ContractEvent`, `--file` for file input). |
| `sdkt inspect <contract-id>` | Inspect a contract's ABI and storage (`--abi <wasm>` for ABI-aware decode). |
| `sdkt storage check <contract-id>` | Storage TTL / rent visibility (`--abi <wasm>`). |
| `sdkt storage analyze <contract-id>` | Classify Instance / Persistent / Temporary storage entries + TTL summary. |
| `sdkt storage estimate <wasm-path>` | Estimate storage cost for a WASM. |
| `sdkt tx inspect <hash>` | Transaction status / ledger inclusion. |
| `sdkt tx simulate <xdr>` | Offline pre-flight via `simulateTransaction`. |
| `sdkt tx submit <xdr>` | Submit a transaction (with optional poll). |
| `sdkt tx build` | Typed envelope builder. |
| `sdkt events <contract-id>` | Emitled-contract event explorer (`--abi <wasm>`). |
| `sdkt account <address>` | Account balances + signers (Horizon-enriched). |
| `sdkt fee estimate` | Dynamic fee estimate from recent ledger base fees. |
| `sdkt wasm metadata <contract>` | WASM metadata for a deployed contract (cached). |
| `sdkt wasm cache` | Manage the WASM cache (`info` / `remove` / `clear`). |
| `sdkt diff --old-wasm <A> --new-wasm <B>` | Offline ABI/function/event/type diff of two WASM files. Add `--upgrade-safety` for a breaking-change verdict. |
| `sdkt audit <path.rs>` | Static security analysis (AUTH-001/002/003, MOVE-001). `--disable <RULE_ID>` to skip a rule. `--rules <path>` (repeatable) to load external rule paths. |
| `sdkt identity <generate|import|list|show|delete|default>` | ED25519 keystore management. |
| `sdkt init <name>` | Scaffold a new Soroban project (`--minimal`, `--force`). |
| `sdkt deploy --wasm <file> --salt <salt>` | Upload WASM + instantiate. Add `--deny-breaking --old-wasm <deployed.wasm>` to abort on a non-backwards-compatible upgrade. |

Most commands accept `--format json` for scripting / CI integration.

## Upgrade Safety in CI

`sdkt` ships a reusable GitHub composite Action. See [`docs/ci-cd.md`](docs/ci-cd.md) for copy-paste workflows (audit-on-PR, upgrade-safety-on-release).

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Testing

CLI integration tests use `assert_cmd` under `crates/sdkt-cli/tests/`. Run `cargo test --workspace`.

## Roadmap

See [`ROADMAP.md`](ROADMAP.md) and the changelog in [`CHANGELOG.md`](CHANGELOG.md).
