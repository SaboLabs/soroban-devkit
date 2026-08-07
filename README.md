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
- **Build & ship** — typed transaction envelope builder, simulate, **native transaction signing (M27)**, submit, identity/keystore management, multi-contract workspace topological deployments, and upgrade breaking-change guards.

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
| Named network profiles | `sdkt network` |
| Project scaffolding | `sdkt init` |
| Deploy (with `--deny-breaking` guard) | `sdkt deploy` |

## Quick Start

Running `sdkt` takes under five minutes. Pick an install method, verify, then
run your first command.

### 1. Install

**Recommended — install.sh (no Rust toolchain needed):**

```bash
curl -fsSL https://raw.githubusercontent.com/naninu123/soroban-devkit/main/install.sh | bash
```

The script detects your OS/arch, downloads the matching release binary,
verifies its SHA-256 checksum, and installs `sdkt` to `~/.local/bin/sdkt`.

**Option A — Manual GitHub Release download:**

1. Open the [Releases](https://github.com/naninu123/soroban-devkit/releases)
   page and download the latest release for your platform:

   | Platform | Asset |
   |----------|-------|
   | Linux (x86_64) | `sdkt-x86_64-unknown-linux-gnu.tar.gz` |
   | macOS (Intel) | `sdkt-x86_64-apple-darwin.tar.gz` |
   | macOS (Apple Silicon) | `sdkt-aarch64-apple-darwin.tar.gz` |

2. Extract and run:

   ```bash
   tar -xzf sdkt-x86_64-unknown-linux-gnu.tar.gz   # Linux x86_64
   chmod +x sdkt
   ./sdkt --version
   # Optional: make it available system-wide
   sudo mv sdkt /usr/local/bin/
   ```

**Option B — Build from source (requires Rust 1.88.0+):**

```bash
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli
sdkt --version
```

### 2. Verify and explore

```bash
sdkt --version
# sdkt <version>

sdkt --help
# Commands:
#   decode    Decode base64-encoded XDR to JSON
#   wasm      Manage WASM metadata and caching
#   diff      Offline diff of two contract WASM files
#   audit     Static security analysis of a Soroban contract source file
#   init      Initialize a new Soroban contract project
#   deploy    Deploy a contract (Upload WASM + Instantiate)
#   ... (run `sdkt --help` to see all)
```

A successful `sdkt --version` means the binary is installed and on your `PATH`.

### Shell completions

Generate completion scripts for your shell and source them:

```bash
# bash
sdkt completions bash > ~/.local/share/bash-completion/completions/sdkt
# zsh
sdkt completions zsh > "${fpath[1]}/_sdkt"
# fish
sdkt completions fish > ~/.config/fish/completions/sdkt.fish
# powershell
sdkt completions powershell > sdkt.ps1
# then in your profile: . ./sdkt.ps1
```

Supported shells: `bash`, `zsh`, `fish`, `powershell` (also `elvish`).

### 3. Your first command (offline)

Inspect a compiled contract WASM that ships with the repo:

```bash
sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_old.wasm
```

Then follow the guided walkthrough in
[docs/quick-start.md](docs/quick-start.md) — it covers inspect, audit, and
upgrade-safety diff step by step.

## Use Cases

- **Inspect Soroban WASM** — read ABI, functions, events, and metadata from any
  compiled contract, offline (`sdkt wasm inspect`).
- **Compare contract upgrades safely** — diff two WASM files and get a
  breaking-change verdict before deploying (`sdkt diff --upgrade-safety`).
- **Audit contracts offline** — static security analysis of contract source with
  no network or secrets (`sdkt audit`).
- **Scaffold new projects** — generate a ready-to-build Soroban contract
  (`sdkt init`).
- **Deploy with upgrade protection** — upload and instantiate, aborting on a
  non-backwards-compatible upgrade (`sdkt deploy --deny-breaking`).

## Installation (details)

Full options — including the `wasm-plugins` / `plugins` feature flags,
installing from crates.io, and updating — are in
[docs/installation.md](docs/installation.md).

### Extensibility & Plugins

The `sdkt audit` static analysis engine supports third-party plugins.

- **`wasm-plugins` (Recommended):** Build with `cargo install --path crates/sdkt-cli --features wasm-plugins` to load platform-independent, sandboxed `.wasm` plugins.
- **`plugins`:** Build with `--features plugins` to load native shared libraries (`.so`, `.dylib`).

See [`docs/plugin-authoring.md`](docs/plugin-authoring.md) for how to build or use custom rules.

## Commands

| Command | Purpose |
|---------|---------|
| `sdkt decode <xdr>` | Decode base64 XDR (`--type ScVal|TransactionEnvelope|ContractEvent`, `--file` for file input). |
| `sdkt inspect <contract-id>` | Inspect a contract's ABI and storage (`--abi <wasm>` for ABI-aware decode). |
| `sdkt storage check <contract-id>` | Storage TTL / rent visibility (`--abi <wasm>`). |
| `sdkt storage analyze <contract-id>` | Classify Instance / Persistent / Temporary storage entries + TTL summary. |
| `sdkt storage estimate <wasm-path>` | Estimate storage cost for a WASM. |
| `sdkt tx inspect <hash>` | Transaction status / ledger inclusion. |
| `sdkt tx validate <xdr>` | Offline pre-flight validation of an envelope (parses + structural checks). |
| `sdkt tx simulate <xdr>` | Offline pre-flight via `simulateTransaction` (RPC). |
| `sdkt tx sign --input <xdr> --identity <name>` | Sign an envelope with a local ED25519 identity — fully offline. |
| `sdkt tx submit <xdr>` | Submit a transaction (with optional poll; RPC). |
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
| `sdkt wasm metadata --contract <contract>` | WASM metadata for a deployed contract (cached). |
| `sdkt wasm cache` | Manage the WASM cache (`info` / `remove` / `clear`). |
| `sdkt audit <path.rs>` | Static security analysis (AUTH-001/002/003, MOVE-001). `--disable <RULE_ID>` to skip a rule. `--rules <path>` (repeatable) to load external rule paths. |
| `sdkt identity <generate\|import\|list\|show\|delete\|default>` | ED25519 keystore management. |
| `sdkt network <add\|list\|show\|remove>` | Named network profiles (RPC URL + passphrase). Combine with `--network-profile <NAME>` on any RPC command to avoid repeating endpoints; `--rpc-url` / `--network-passphrase` override. |
| `sdkt init <name>` | Scaffold a new Soroban project (`--minimal`, `--force`). |
| `sdkt build` | Compile workspace Rust contracts into optimized WASMs. |
| `sdkt lock generate` | Write `sdkt.lock` recording each built artifact's SHA-256 + deploy order (after `sdkt build`). |
| `sdkt lock verify` | Check `sdkt.lock` against current on-disk artifacts (advisory; never fails the build). |
| `sdkt lock show` | Print the current `sdkt.lock` contents. |

### Multi-contract dependency graphs

A `.sdkt.toml` workspace declares contracts under `[contracts.<alias>]`. Each
contract may declare dependencies using either the canonical `depends_on`
(M34.2) or the legacy `deploy_after` field — both are merged during
resolution:

```toml
[contracts.token]
path = "contracts/token"

[contracts.router]
path = "contracts/router"
depends_on = ["token"]
```

`sdkt build`, `sdkt project deploy`, and `sdkt lock generate` all resolve the
same dependency graph via a single topological sort (Kahn's algorithm), so the
deploy order is deterministic and identical across commands. Invalid graphs are
rejected up front with a clear error:

- **Unknown dependency** — a `depends_on`/`deploy_after` entry that is not a
  defined `[contracts.*]`.
- **Self-dependency** — a contract that lists itself as a dependency.
- **Duplicate dependency** — the same dependency declared more than once
  (e.g. in both `depends_on` and `deploy_after`).
- **Circular dependency** — a cycle such as `a → b → a`.
- **Duplicate contract name** — two `[contracts.<alias>]` tables (TOML parse
  error, surfaced instead of silently defaulting to an empty config).
| `sdkt deploy --wasm <file> --salt <salt>` | Upload WASM + instantiate. Add `--deny-breaking --old-wasm <deployed.wasm>` to abort on a non-backwards-compatible upgrade. |

### Network profiles

Save an RPC endpoint once and reference it from any RPC command instead of
repeating full URLs:

```bash
# Save a profile
sdkt network add testnet \
  --rpc-url https://soroban-testnet.stellar.org \
  --passphrase "Test SDF Network ; September 2015"

# Use it (any RPC command)
sdkt inspect <CONTRACT_ID> --network-profile testnet
sdkt account <ADDRESS> --network-profile testnet
```

Every RPC command (`inspect`, `verify`, `health`, `storage`, `events`, `account`,
`tx`, `fee`, `wasm`, `deploy`, `project deploy`) also accepts explicit
`--rpc-url <URL>` and `--network-passphrase <PASSPHRASE>` override flags.
**Precedence (highest wins):** explicit `--rpc-url` / `--network-passphrase` >
`--network-profile` > `.sdkt.toml` `[network]` > built-in testnet default.
Commands without these flags behave exactly as before. (`tx sign` is offline and
excluded.) See [docs/cli.md](docs/cli.md) and [docs/examples.md](docs/examples.md).

Most commands accept `--format json` for scripting / CI integration.

## Common Workflows

- **Audit every PR** — gate merges on `sdkt audit` (fails on `critical`). See
  [docs/ci-cd.md](docs/ci-cd.md).
- **Safe upgrades** — run `sdkt diff --upgrade-safety` in release CI to block
  breaking contract changes.
- **Local analysis** — `decode`, `diff`, and `audit` need no RPC; run them in
  CI or locally without secrets.

### End-to-end transaction signing (build → sign → submit)

`sdkt` can build, validate, simulate, **sign**, and submit a Soroban
transaction entirely from the CLI. Signing is **fully offline** — it uses a
local ED25519 identity from the keystore (`sdkt identity`); no RPC, no secret
ever leaves your machine.

```bash
# 1. Create a local signing identity (offline)
sdkt identity generate alice

# 2. Build an unsigned envelope (offline)
sdkt tx build \
  --source <SOURCE_ACCOUNT> \
  --sequence <SEQ> \
  --contract <CONTRACT_ID> \
  --function hello \
  --output unsigned.xdr

# 3. Validate the envelope offline
sdkt tx validate --envelope unsigned.xdr

# 4. Simulate against the network (RPC) to catch failures early
sdkt tx simulate --envelope unsigned.xdr

# 5. Sign with the local identity (offline) — picks testnet by default
sdkt tx sign --input unsigned.xdr --output signed.xdr --identity alice --network testnet

# 6. Submit the signed envelope (RPC)
sdkt tx submit --envelope signed.xdr
```

Notes:
- `tx sign` takes `--network testnet|mainnet|futurenet|custom:<passphrase>`; the
  network only affects the signature hash, so signing needs no RPC.
- `tx submit` / `tx simulate` read the network from `.sdkt.toml`
  (`[network]` section) or fall back to the default testnet RPC.
- `--output` is optional; without it, the signed base64 envelope is printed to
  stdout. Use `--format json` for scripting.

Copy-paste recipes for every subcommand are in
[docs/examples.md](docs/examples.md).

## Upgrade Safety in CI

`sdkt` ships a reusable GitHub composite Action. See
[docs/ci-cd.md](docs/ci-cd.md) for copy-paste workflows (audit-on-PR,
upgrade-safety-on-release).

## Documentation

- [docs/quick-start.md](docs/quick-start.md) — five-minute first-time walkthrough.
- [docs/getting-started.md](docs/getting-started.md) — deeper offline `diff` and `audit` examples.
- [docs/examples.md](docs/examples.md) — command recipes & CI gating.
- [docs/installation.md](docs/installation.md) — build / install / features.
- [docs/compatibility.md](docs/compatibility.md) — real-world contract compatibility matrix.
- [docs/ci-cd.md](docs/ci-cd.md) — CI/CD with the reusable Action.
- [SECURITY.md](SECURITY.md) — supported versions and vulnerability reporting.
- [CONTRIBUTING.md](CONTRIBUTING.md) — how to contribute.

Additional references: [docs/cli.md](docs/cli.md) (full command reference),
[docs/faq.md](docs/faq.md) (FAQ),
[docs/plugin-authoring.md](docs/plugin-authoring.md) (write your own audit
rules), and [ROADMAP.md](ROADMAP.md) · [CHANGELOG.md](CHANGELOG.md) ·
[GAP_ANALYSIS.md](GAP_ANALYSIS.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All contributions are welcome — docs,
tests, and small fixes are great first PRs. Please follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

[MIT](LICENSE).
