# Soroban DevKit (`sdkt`)

[![CI](https://github.com/naninu123/soroban-devkit/actions/workflows/ci.yml/badge.svg)](https://github.com/naninu123/soroban-devkit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/naninu123/soroban-devkit?label=release)](https://github.com/naninu123/soroban-devkit/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`sdkt` is a unified, offline-capable toolkit for Stellar / Soroban
development. It unifies contract inspection, XDR decoding, storage TTL
analysis, ABI-aware decoding, static security analysis, WASM diffing, and an
upgrade-safety verdict into a single CLI — instead of juggling 5+ separate
tools.

## Overview

`sdkt` spans the full read-only **and** mutating contract lifecycle:

- **Inspect & decode** — base64 XDR decoding, contract ABI + storage
  inspection, event exploration.
- **Analyze** — storage TTL / rent visibility, Instance / Persistent /
  Temporary classification, offline ABI/function/event/type WASM diffing.
- **Secure** — static analysis of contract source (`AUTH-001/002/003`,
  `MOVE-001`) and an upgrade-safety verdict for safe contract upgrades.
- **Build & ship** — typed transaction envelope builder, simulate, submit,
  identity/keystore management, project scaffolding, and deploy with an
  optional breaking-change guard.

Most commands are **offline**; only on-chain reads (`inspect`, `storage`,
`tx`, `events`, `account`, `fee`, `wasm metadata`) need an RPC endpoint.

## Feature Highlights

| Capability | Command |
|------------|---------|
| Decode base64 XDR (`ScVal`, `TransactionEnvelope`, `ContractEvent`) | `sdkt decode` |
| Inspect contract ABI + storage | `sdkt inspect`, `sdkt storage check` |
| Storage TTL / rent analysis | `sdkt storage analyze`, `sdkt storage estimate` |
| Transaction inspect / simulate / submit / build | `sdkt tx *` |
| Event explorer | `sdkt events` |
| Account balances + signers | `sdkt account` |
| Dynamic fee estimate | `sdkt fee estimate` |
| WASM Operations | `sdkt wasm inspect`, `sdkt wasm metadata`, `sdkt wasm cache` |
| Offline WASM diff + upgrade-safety verdict | `sdkt diff --upgrade-safety` |
| Static security audit | `sdkt audit` |
| ED25519 keystore | `sdkt identity` |
| Project scaffolding | `sdkt init` |
| Deploy (with `--deny-breaking` guard) | `sdkt deploy` |

## Installation

**Prerequisites:** Rust `1.88.0` or higher is required.

```bash
# Build from source
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli

# Verify
sdkt --version
```

### Extensibility & Plugins

The `sdkt audit` static analysis engine supports third-party plugins.

- **`wasm-plugins` (Recommended):** Build with `cargo install --path crates/sdkt-cli --features wasm-plugins` to load platform-independent, sandboxed `.wasm` plugins.
- **`plugins`:** Build with `--features plugins` to load native shared libraries (`.so`, `.dylib`).

See [`docs/plugin-authoring.md`](docs/plugin-authoring.md) for how to build or use custom rules.

Full options (features, from crates.io, updating) are in
[docs/installation.md](docs/installation.md).

## Getting Started

New here? Start with [docs/getting-started.md](docs/getting-started.md) — it
walks you through your first offline `diff` and `audit` in under five minutes.

### Quick Start

Offline ABI/WASM diff (no network needed):

```bash
sdkt diff \
  --old-wasm crates/sdkt-cli/tests/fixtures/us_old.wasm \
  --new-wasm crates/sdkt-cli/tests/fixtures/us_new.wasm
```

Static security audit of a contract:

```bash
sdkt audit contracts/token/src/lib.rs
```

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
| `sdkt events <contract-id>` | Emitted-contract event explorer (`--abi <wasm>`). |
| `sdkt account <address>` | Account balances + signers (Horizon-enriched). |
| `sdkt diff` | Offline comparison of WASM binaries and API surfaces. |
| `sdkt wasm inspect <file>` | Inspect offline WASM metadata, sections, and specifications. |
| `sdkt wasm metadata <contract>` | WASM metadata for a deployed contract (cached). |
| `sdkt wasm cache` | Manage the WASM cache (`info` / `remove` / `clear`). |
| `sdkt diff --old-wasm <A> --new-wasm <B>` | Offline ABI/function/event/type diff of two WASM files. Add `--upgrade-safety` for a breaking-change verdict. |
| `sdkt audit <path.rs>` | Static security analysis (AUTH-001/002/003, MOVE-001). `--disable <RULE_ID>` to skip a rule. `--rules <path>` (repeatable) to load external rule paths. |
| `sdkt identity <generate\|import\|list\|show\|delete\|default>` | ED25519 keystore management. |
| `sdkt init <name>` | Scaffold a new Soroban project (`--minimal`, `--force`). |
| `sdkt deploy --wasm <file> --salt <salt>` | Upload WASM + instantiate. Add `--deny-breaking --old-wasm <deployed.wasm>` to abort on a non-backwards-compatible upgrade. |

Most commands accept `--format json` for scripting / CI integration.

## Common Workflows

- **Audit every PR** — gate merges on `sdkt audit` (fails on `critical`). See
  [docs/ci-cd.md](docs/ci-cd.md).
- **Safe upgrades** — run `sdkt diff --upgrade-safety` in release CI to block
  breaking contract changes.
- **Local analysis** — `decode`, `diff`, and `audit` need no RPC; run them in
  CI or locally without secrets.

Copy-paste recipes for every subcommand are in
[docs/examples.md](docs/examples.md).

## Upgrade Safety in CI

`sdkt` ships a reusable GitHub composite Action. See
[docs/ci-cd.md](docs/ci-cd.md) for copy-paste workflows (audit-on-PR,
upgrade-safety-on-release).

## Documentation

- [docs/getting-started.md](docs/getting-started.md) — five-minute onboarding.
- [docs/installation.md](docs/installation.md) — build / install / features.
- [docs/examples.md](docs/examples.md) — command recipes & CI gating.
- [docs/faq.md](docs/faq.md) — frequently asked questions.
- [docs/cli.md](docs/cli.md) — full command reference.
- [docs/ci-cd.md](docs/ci-cd.md) — CI/CD with the reusable Action.
- [docs/plugin-authoring.md](docs/plugin-authoring.md) — write your own audit rules.
- [ROADMAP.md](ROADMAP.md) · [CHANGELOG.md](CHANGELOG.md) · [GAP_ANALYSIS.md](GAP_ANALYSIS.md)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All contributions are welcome — docs,
tests, and small fixes are great first PRs. Please follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

[MIT](LICENSE).
