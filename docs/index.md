# Soroban DevKit

**Soroban DevKit** (`sdkt`) is an offline-first CLI and Rust toolkit for
building, inspecting, auditing, and deploying
[Stellar / Soroban](https://soroban.stellar.org) smart contracts. It
consolidates contract inspection, XDR decoding, storage analysis, static
security auditing, WASM diffing, transaction lifecycle management, and
multi-contract deployment orchestration into a single command-line
interface — so developers stop juggling 5+ separate tools.

The workspace is a Rust workspace published as `sdkt-cli` (binary name:
`sdkt`). Most commands run fully offline; only on-chain operations require
an RPC endpoint.

## What can you do with sdkt?

| Area | Capabilities |
|------|--------------|
| **Project workflow** | Scaffold new Soroban projects (`init`), compile contracts to WASM (`build`), inspect WASM artifacts offline (`wasm inspect`) |
| **ABI / ContractSpec** | Decode base64 XDR (`decode`), inspect contract ABI and storage (`inspect`), diff two WASM files for upgrade safety (`diff`) |
| **Security** | Static analysis of contract source with built-in rules (`audit`), plus a plugin system for custom rules |
| **Transactions** | Build, validate, simulate, sign (offline ED25519), and submit transactions (`tx *`); one-command state-changing invoke (`invoke`) |
| **Deployment** | Upload WASM + instantiate contracts (`deploy`) with optional `--deny-breaking` upgrade guard; multi-contract workspace orchestration (`project deploy`) |
| **Contract interaction** | Read-only calls (`call`) and state-changing invocations (`invoke`) with typed arguments and ABI-aware result decoding |
| **Events & storage** | Event explorer (`events`), storage TTL analysis and extension (`storage *`) |
| **Network** | Named network profiles for RPC endpoints + passphrases (`network *`) |
| **Plugins** | Local, offline-first plugin store with `.sdktplugin` bundle support (`plugin *`) |
| **Identity** | ED25519 keystore management and Testnet Friendbot funding (`identity *`) |

## Start here

1. **[Installation](installation.md)** — install `sdkt` from a release binary, crates.io, or source.
2. **[Quick Start](quick-start.md)** — five-minute first-time walkthrough (inspect, audit, diff, sign).
3. **[Getting Started](getting-started.md)** — deeper offline workflow examples.
4. **[Examples & Common Workflows](examples.md)** — copy-paste recipes for every subcommand.

## Documentation

| Topic | Reference |
|-------|-----------|
| Full CLI reference | [CLI Command Reference](cli.md) |
| Static security audit | [Audit rules & plugin authoring](plugin-authoring.md) |
| Deployment & transactions | [Examples — Deploy](examples.md#deploy) · [Examples — Transaction lifecycle](examples.md#transaction-lifecycle) |
| ABI / ContractSpec | [Examples — Inspect a contract's ABI and storage](examples.md#inspect-a-contracts-abi-and-storage) · [WASM commands](cli.md#command-tree) |
| Events & storage | [Examples — Events and account](examples.md#events-and-account) · [Storage commands](cli.md#command-tree) |
| Plugin system | [Plugin Authoring](plugin-authoring.md) · [Signed `.sdktplugin` bundles](plugin-bundles.md) |
| Compatibility | [Compatibility Matrix](compatibility.md) · [Compatibility CI](ci-compatibility.md) |
| CI / CD | [CI/CD with reusable Action](ci-cd.md) |
| FAQ | [FAQ](faq.md) |
| Adoption evidence | [Adoption and Integration Evidence](adoption.md) |
| Release history | [Releases](releases/v0.6.0-alpha.md) · [CHANGELOG](../CHANGELOG.md) |

## Who is this for?

Soroban DevKit is for developers building on **Stellar / Soroban** who want
a single, scriptable, offline-first toolchain for the full contract
lifecycle — from scaffolding and local analysis through auditing,
deployment, and on-chain interaction. It is especially useful for teams
that want reproducible CI gating (audit + upgrade safety) and multi-contract
workspace orchestration.

## Project

- **Source:** [github.com/SaboLabs/soroban-devkit](https://github.com/SaboLabs/soroban-devkit)
- **Website:** [sabolabs.github.io/soroban-devkit](https://sabolabs.github.io/soroban-devkit/) — landing page + in-browser WASM inspector (contract bytes stay in the tab)
- **License:** MIT
