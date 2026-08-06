# Soroban DevKit (`sdkt`)

[![CI](https://github.com/naninu123/soroban-devkit/actions/workflows/ci.yml/badge.svg)](https://github.com/naninu123/soroban-devkit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/naninu123/soroban-devkit?label=release)](https://github.com/naninu123/soroban-devkit/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`sdkt` is a unified, offline-capable toolkit for Stellar / Soroban development. It consolidates the fragmented developer lifecycle—contract inspection, XDR decoding, storage TTL analysis, static security analysis, WASM diffing, and multi-contract deployment orchestration—into a single, production-grade CLI.

## The Problem
Developing on Soroban often requires context-switching across multiple CLI tools and manual RPC scripts to securely build, audit, and deploy contracts. `sdkt` solves this by providing a unified interface that emphasizes **offline-first** analysis, **upgrade safety**, and **production deployment orchestration**.

## Capabilities

`sdkt` spans the full read-only **and** mutating contract lifecycle:

- **Inspect & decode** — base64 XDR decoding, contract ABI + storage inspection, event exploration.
- **Analyze** — storage TTL / rent visibility, Instance / Persistent / Temporary classification, offline ABI/function/event/type WASM diffing.
- **Secure** — static analysis of contract source (`AUTH-001/002/003`, `MOVE-001`) and an upgrade-safety verdict for safe contract upgrades.
- **Build & ship** — typed transaction envelope builder, simulate, submit, identity/keystore management, multi-contract workspace topological deployments, and upgrade breaking-change guards.

Most commands are **offline**; only on-chain reads (`inspect`, `storage`, `tx`, `events`, `account`, `fee`, `wasm metadata`) need an RPC endpoint.

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
| Multi-contract Orchestration | `sdkt build`, `sdkt project deploy` |
| WASM Operations | `sdkt wasm inspect`, `sdkt wasm metadata`, `sdkt wasm cache`, `sdkt verify`, `sdkt health` |
| Offline WASM diff + upgrade-safety verdict | `sdkt diff --upgrade-safety` |
| Static security audit | `sdkt audit` |
| ED25519 keystore | `sdkt identity` |
| Project scaffolding | `sdkt init` |
| Deploy (with `--deny-breaking` guard) | `sdkt deploy` |

## Quick Start

Get `sdkt` running in under five minutes. Pick one install method, verify,
then run your first command.

### 1. Install

**Option A — GitHub Release binary (fastest, no Rust toolchain needed):**

Download the asset matching your platform from the
[Releases](https://github.com/naninu123/soroban-devkit/releases) page. Asset
names follow `sdkt-<target>.tar.gz` (each includes the `sdkt` binary and a
`sdkt.sha256` checksum).

```bash
# Example: Linux x86_64
VERSION=$(curl -fsSL https://api.github.com/repos/naninu123/soroban-devkit/releases/latest | grep -oE '"tag_name": *"[^"]+"' | head -1 | cut -d'"' -f4)
curl -fsSL -o sdkt.tar.gz "https://github.com/naninu123/soroban-devkit/releases/download/${VERSION}/sdkt-x86_64-unknown-linux-gnu.tar.gz"
tar -xzf sdkt.tar.gz
./sdkt --version
# (optional) make it available system-wide:
sudo mv sdkt /usr/local/bin/
```

> Other targets: `x86_64-apple-darwin` (macOS Intel) and
> `aarch64-apple-darwin` (macOS Apple Silicon). Replace the filename in the
> `curl` command accordingly.

**Option B — Build from source (requires Rust):**

**Prerequisites:** Rust `1.88.0` or higher.

```bash
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli

# Verify
sdkt --version
```

### 2. Your first useful command

```bash
sdkt --help
```

This prints every available command and subcommand. Everything from here is
offline unless explicitly noted.

### 3. A real first action (offline)

Inspect a compiled contract WASM:

```bash
sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_old.wasm
```

Or, for a guided walkthrough, continue to
[docs/quick-start.md](docs/quick-start.md) — a step-by-step first-time guide
that covers inspect, audit, and upgrade-safety diff.

## Installation (details)

Full options — including the `wasm-plugins` / `plugins` feature flags,
installing from crates.io, and updating — are in
[docs/installation.md](docs/installation.md).

### Extensibility & Plugins

The `sdkt audit` static analysis engine supports third-party plugins.

- **`wasm-plugins` (Recommended):** Build with `cargo install --path crates/sdkt-cli --features wasm-plugins` to load platform-independent, sandboxed `.wasm` plugins.
- **`plugins`:** Build with `--features plugins` to load native shared libraries (`.so`, `.dylib`).

See [`docs/plugin-authoring.md`](docs/plugin-authoring.md) for how to build or use custom rules.

## Getting Started

New here? Start with [docs/quick-start.md](docs/quick-start.md) for the
five-minute first-time walkthrough, then
[docs/getting-started.md](docs/getting-started.md) for deeper offline `diff`
and `audit` examples.

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
| `sdkt diff --old-wasm <A> --new-wasm <B>` | Offline ABI/function/event/type diff of two WASM files. Add `--upgrade-safety` for a breaking-change verdict. |
| `sdkt build` | Compile workspace rust contracts into optimized WASMs. |
| `sdkt project deploy` | Deploy multi-contract workspace orchestrating topological dependency sorting. |
| `sdkt verify --contract <ID> [--wasm <file>] [--network <net>]` | Verify a deployed contract matches a local WASM (offline hash vs on-chain hash). |
| `sdkt health --contract <ID> [--wasm <file>] [--network <net>]` | Unified read-only contract posture report (WASM, storage, TTL, health verdict). |
| `sdkt wasm inspect <file>` | Inspect offline WASM metadata, sections, and specifications. |
| `sdkt wasm metadata <contract>` | WASM metadata for a deployed contract (cached). |
| `sdkt wasm cache` | Manage the WASM cache (`info` / `remove` / `clear`). |
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
