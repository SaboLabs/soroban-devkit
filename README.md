<h1 align="center">Soroban DevKit (<code>sdkt</code>)</h1>

<p align="center">
  <a href="https://github.com/SaboLabs/soroban-devkit/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/SaboLabs/soroban-devkit/ci.yml?branch=main&label=CI" alt="CI"></a>
  <a href="https://github.com/SaboLabs/soroban-devkit/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/SaboLabs/soroban-devkit/release.yml?branch=main&label=Release" alt="Release"></a>
  <a href="https://img.shields.io/crates/v/sdkt-cli"><img src="https://img.shields.io/crates/v/sdkt-cli?label=crates.io" alt="crates.io"></a>
  <a href="https://github.com/SaboLabs/soroban-devkit/releases"><img src="https://img.shields.io/github/v/release/SaboLabs/soroban-devkit?label=release" alt="release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

<p align="center">
  Offline-first CLI &amp; Rust toolkit for <strong>Stellar/Soroban</strong>.<br>
  Inspect, decode, analyze, audit, diff, build, and deploy smart contracts with upgrade safety.
</p>

<p align="center">
  <strong>Website:</strong> <a href="https://sabolabs.github.io/soroban-devkit/">sabolabs.github.io/soroban-devkit</a> — landing +
  <a href="https://sabolabs.github.io/soroban-devkit/playground/">in-browser WASM inspector</a>
  (contract bytes stay in the tab). Source: <a href="website/"><code>website/</code></a>.
</p>

---

`sdkt` is an offline-first toolkit for inspecting, analyzing, validating, and managing Soroban smart contracts. It consolidates contract inspection, XDR decoding, storage TTL analysis, static security analysis, WASM diffing, and multi-contract deployment orchestration into a single CLI — so developers stop juggling 5+ separate tools.

## The Problem

Developing on Soroban often requires context-switching across multiple CLI tools and manual RPC scripts to build, audit, and deploy contracts. `sdkt` solves this by providing a unified interface that emphasizes **offline-first** analysis, **upgrade safety**, and **deployment orchestration**.

## Capabilities

`sdkt` spans the full read-only **and** mutating contract lifecycle:

- **Inspect & decode** — base64 XDR decoding, contract ABI + storage inspection, event exploration.
- **Analyze** — storage TTL / rent visibility, Instance / Persistent / Temporary classification, offline ABI/function/event/type WASM diffing.
- **Secure** — static analysis of contract source (`AUTH-001/002/003/004`, `MOVE-001`) and an upgrade-safety verdict for safe contract upgrades.
- **Build & ship** — typed transaction envelope builder, simulate, **native transaction signing**, submit, identity/keystore management, multi-contract workspace topological deployments, and upgrade breaking-change guards.

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
| Contract invocation (read-only) | `sdkt call` |
| Contract invocation (state-changing, end-to-end) | `sdkt invoke` |

## Quick Start

Running `sdkt` takes under five minutes. Pick an install method, verify, then
run your first command.

### 1. Install

Choose **one** of the following methods.

#### Recommended — install.sh (no Rust toolchain needed)

```bash
curl -fsSL https://raw.githubusercontent.com/SaboLabs/soroban-devkit/main/install.sh | bash
```

The script detects your OS/arch, downloads the matching release binary,
verifies its SHA-256 checksum, and installs `sdkt` to `~/.local/bin/sdkt`.

#### Alternative — Manual GitHub Release download

1. Open the [Releases](https://github.com/SaboLabs/soroban-devkit/releases)
   page and download the latest release for your platform:

   | Platform | Asset |
   |----------|-------|
   | Linux (x86_64) | `sdkt-x86_64-unknown-linux-gnu.tar.gz` |
   | Linux (aarch64) | not in v2.5.0 Release — use `install.sh` or `cargo install sdkt-cli` |
   | macOS (Intel) | `sdkt-x86_64-apple-darwin.tar.gz` |
   | macOS (Apple Silicon) | `sdkt-aarch64-apple-darwin.tar.gz` |

   Windows x86_64 is not included in the v2.5.0 GitHub Release. Windows
   users can install via `cargo install sdkt-cli` or build from source.

2. Extract and run:

   **Linux/macOS:**

   ```bash
   tar -xzf sdkt-x86_64-unknown-linux-gnu.tar.gz   # Linux x86_64
   chmod +x sdkt
   ./sdkt --version
   # Optional: make it available system-wide
   sudo mv sdkt /usr/local/bin/
   ```

   **Windows (v2.5.0):** no GitHub Release zip yet. Use crates.io or source:

   ```powershell
   cargo install sdkt-cli
   sdkt --version
   ```

#### Alternative — Build from source (requires Rust 1.88.0+)

```bash
git clone https://github.com/SaboLabs/soroban-devkit
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

**No install:** drop a `.wasm` on the
[Web Playground](https://sabolabs.github.io/soroban-devkit/playground/)
(ContractSpec / exports / hash; bytes never leave the browser).

**CLI:** inspect a compiled contract WASM that ships with the repo:

```bash
sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_old.wasm
```

Then follow the guided walkthrough in
[docs/quick-start.md](docs/getting-started/quick-start.md) — it covers inspect, audit, and
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

### From scaffold to deployment

A typical first-contract workflow:

```bash
# 1. Create a new contract project
sdkt init my-contract --minimal
cd my-contract

# 2. Build the contract into a Soroban WASM
#    Output: target/wasm32-unknown-unknown/release/<project>.wasm
sdkt build

# 3. Generate a local signing identity
sdkt identity generate my-deployer

# 4. Configure Testnet (one-time setup)
sdkt network add testnet \
  --rpc-url https://soroban-testnet.stellar.org \
  --passphrase "Test SDF Network ; September 2015" \
  --friendbot https://friendbot.stellar.org

# 5. Fund your identity (Testnet only)
#    Copy the public key from `sdkt identity show my-deployer` and paste it at:
#    https://friendbot.stellar.org
#    OR use the CLI directly:
sdkt identity fund my-deployer --network-profile testnet

# 6. Generate a 20-byte salt (40 hex characters)
openssl rand -hex 20

# 7. Deploy to Testnet
sdkt deploy \
  --wasm target/wasm32-unknown-unknown/release/my_contract.wasm \
  --salt <paste-hex-from-step-6> \
  --identity my-deployer \
  --network-profile testnet

# 8. Invoke a contract function (state-changing: sequence → simulate → sign → submit → poll)
sdkt invoke <CONTRACT_ID> increment --args u32:1 --identity my-deployer --network-profile testnet
```

For a detailed explanation of each step, see [Deploy a single contract](#deploy-a-single-contract).

## Installation (details)

Full options — including the `wasm-plugins` / `plugins` feature flags,
installing from crates.io, and updating — are in
[docs/installation.md](docs/getting-started/installation.md).

### Extensibility & Plugins

The `sdkt audit` static analysis engine supports third-party plugins.

- **`wasm-plugins` (Recommended):** Build with `cargo install --path crates/sdkt-cli --features wasm-plugins` to load platform-independent, sandboxed `.wasm` plugins.
- **`plugins`:** Build with `--features plugins` to load native shared libraries (`.so`, `.dylib`).

See [`docs/plugin-authoring.md`](docs/plugins/plugin-authoring.md) for how to build or use custom rules.

## Commands

| Command | Purpose |
|---------|---------|
| `sdkt decode <xdr>` | Decode base64 XDR (`--type ScVal|TransactionEnvelope|ContractEvent`, `--file` for file input). |
| `sdkt inspect <contract-id>` | Inspect a contract's ABI and storage (`--abi <wasm>` for ABI-aware decode). |
| `sdkt storage check <contract-id>` | Storage TTL / rent visibility (`--abi <wasm>`). |
| `sdkt storage analyze <contract-id>` | Classify Instance / Persistent / Temporary storage entries + TTL summary. |
| `sdkt storage estimate <wasm-path>` | Estimate storage cost for a WASM. |
| `sdkt storage read <contract-id> --key-xdr <BASE64_XDR>` | Read a contract storage entry by its complete LedgerKey. ABI optional for ScVal formatting. |
| `sdkt storage extend <contract-id> --ledgers <N>` | Extend TTL of known footprint keys (`ExtendFootprintTtl`). Instance key is always included; extra keys via `--key`. Does not restore archived entries. |
| `sdkt tx inspect <hash>` | Transaction status / ledger inclusion. |
| `sdkt tx validate --envelope <xdr>` | Offline pre-flight validation of an envelope (parses + structural checks). |
| `sdkt tx simulate <xdr>` | Offline pre-flight via `simulateTransaction` (RPC). `--abi <wasm>` decodes the invoke result via the contract spec. |
| `sdkt tx sign --input <xdr> --identity <name>` | Sign an envelope with a local ED25519 identity — fully offline. |
| `sdkt tx submit <xdr>` | Submit a transaction (with optional poll; RPC). |
| `sdkt tx build` | Typed envelope builder. |
| `sdkt events <contract-id>` | Emitted-contract event explorer (`--abi <wasm>`). |
| `sdkt account <address>` | Account balances + signers (Horizon-enriched). |
| `sdkt call <contract> <function> [--args TYPE:VALUE...]` | Read-only contract invocation. No signing, no submission. Returns result + events. `--abi <wasm>` decodes the result via the contract spec; `--abi-contract <id>` fetches the deployed contract's on-chain WASM instead. |
| `sdkt diff` | Offline comparison of WASM binaries and API surfaces. |
| `sdkt diff --old-wasm <A> --new-wasm <B>` | Offline ABI/function/event/type diff of two WASM files. Add `--upgrade-safety` for a breaking-change verdict. |
| `sdkt build` | Compile workspace rust contracts into optimized WASMs. |
| `sdkt project deploy` | Deploy multi-contract workspace orchestrating topological dependency sorting. |
| `sdkt verify --contract <ID> [--wasm <file>] [--network <net>]` | Verify a deployed contract matches a local WASM (offline hash vs on-chain hash). |
| `sdkt health --contract <ID> [--wasm <file>] [--network <net>]` | Unified read-only contract posture report (WASM, storage, TTL, health verdict). |
| `sdkt wasm inspect <file>` | Inspect offline WASM metadata, sections, and specifications. |
| `sdkt wasm metadata --contract <contract>` | WASM metadata for a deployed contract (cached). |
| `sdkt wasm cache` | Manage the WASM cache (`info` / `remove` / `clear`). |
|| `sdkt audit <path.rs>` | Static security analysis (AUTH-001/002/003/004, MOVE-001). `--disable <RULE_ID>` to skip a rule. `--rules <path|id>` (repeatable) to load external rule paths or resolve installed plugin IDs. |
| `sdkt identity <generate\|import\|list\|show\|delete\|default>` | ED25519 keystore management. |
| `sdkt identity fund <name> --network-profile <NAME>` | Fund an identity via Stellar Testnet Friendbot. |
| `sdkt network <add\|list\|show\|remove>` | Named network profiles (RPC URL + passphrase). Combine with `--network-profile <NAME>` on any RPC command to avoid repeating endpoints; `--rpc-url` / `--network-passphrase` override. |
|| `sdkt init <name>` | Scaffold a new Soroban project (`--minimal`, `--force`). |
| `sdkt lock generate` | Write `sdkt.lock` recording each built artifact's SHA-256 + deploy order (after `sdkt build`). |
| `sdkt lock verify` | Verify `sdkt.lock` against current on-disk artifacts **and** package dependencies (lock matches manifest, git commits, path existence). Advisory; never fails the build. Prints `✓ lock file verified` / `✓ package dependencies verified` or lists drift. |
| `sdkt lock show` | Print the current `sdkt.lock` contents. |
| `sdkt package validate` | Validate the local package manifest (`[package]` metadata + local/git `[dependencies]` graph). Offline; never touches the network or a registry. |
| `sdkt package fetch [--force]` | Fetch declared dependencies into `.sdkt-cache` (local path passthrough; git clone/checkout). Never builds. `--force` updates existing checkouts. |
| `sdkt package update [--check] [--dry-run] [--format pretty\|json]` | Synchronize dependencies: refresh git deps to the latest available commit, rewrite `sdkt.lock`. `rev` stays pinned; `tag`/`branch` update on drift. `--check` reports only; `--dry-run` previews without touching cache/lock. |

### Deploy a single contract

`sdkt deploy` uploads a Soroban WASM to the network and instantiates it as a
new contract instance. It is the single-contract counterpart to
`sdkt project deploy` (which orchestrates multi-contract workspaces).

#### Deploy a contract to Testnet (step by step)

A fresh deploy requires a built WASM, a funded signing identity, and a salt.
Follow these steps in order:

```bash
# 1. Build your contract into a Soroban WASM
#    (from a project created with `sdkt init <name>`)
sdkt build

# 2. Generate a local ED25519 identity (used to sign the deploy transactions)
sdkt identity generate my-deployer

# 3. Configure the Testnet network profile
sdkt network add testnet \
  --rpc-url https://soroban-testnet.stellar.org \
  --passphrase "Test SDF Network ; September 2015" \
  --friendbot https://friendbot.stellar.org

# 4. Fund your identity on Testnet
#    Copy the public key from `sdkt identity show my-deployer` and paste it at:
#    https://friendbot.stellar.org
#    (Friendbot is a Testnet faucet — it does NOT work on Mainnet.)

# 5. Deploy (salt is auto-generated if omitted)
sdkt deploy \
  --wasm target/wasm32-unknown-unknown/release/<project>.wasm \
  --identity my-deployer \
  --network-profile testnet

# Or with explicit salt for deterministic address:
# openssl rand -hex 20
# sdkt deploy --wasm ... --salt <40-hex-chars> --identity my-deployer --network-profile testnet
```

Replace `<project>` with your crate name (the `name` field in `Cargo.toml`).
The `--wasm` path follows the Cargo convention
`target/<profile>/<crate>.wasm` — adjust if your build profile or target
directory differs.

#### Options
- `--wasm` (required): Path to a compiled Soroban WASM (32kb+ after `stellar contract build`).
- `--salt` (optional): 40-character hex string (20 bytes). Auto-generated if omitted. It salts the contract
  ID derivation so the same deployer + WASM + salt always yields the same contract
  address across networks. Provide explicitly for deterministic/reproducible deployment.
- `--identity <name>`: ED25519 identity from `sdkt identity` used to sign both
  transactions (upload + instantiate). Defaults to `default`.
- `--format <pretty|json>`: Output format (default `pretty`).
- `--deny-breaking`: Abort if the new WASM is not backwards-compatible with the
  currently deployed contract. Requires `--old-wasm`.
- `--old-wasm <file>`: Path to the existing on-chain baseline WASM. Used only
  with `--deny-breaking`.

#### Flow
1. Resolve the network (flag → profile → `.sdkt.toml` → built-in Testnet default).
2. Apply the mainnet-safety guard (mainnet requires explicit network selection).
3. Optionally run the `--deny-breaking` upgrade-safety check.
4. Load the signing identity and its key.
5. **Upload** — build an `UploadContractWasm` transaction, simulate, apply the
   returned `SorobanTransactionData` and authorization entries, sign, submit,
   and poll for confirmation.
6. **Instantiate** — build a `CreateContract` transaction with the WASM hash from
   the upload and the user-supplied salt, simulate, finalize, sign, submit,
   and poll for confirmation.
7. Derive the contract ID from `Hash(networkId || HashIdPreimage::ContractId{...})`
   and report the deployment result.

#### Safety
- Mainnet deployments are refused unless the operator explicitly selects the
  network (via `--rpc-url` + `--network-passphrase`, or `--network-profile` whose
  passphrase is mainnet). Implicit Testnet defaults cannot touch mainnet.
- The salt must be exactly 40 hex characters; any other length or non-hex input
  is rejected before any network call.

#### Output
```
Deployment Result:
  WASM Hash: e71250b5...
  Contract ID: 0bde4658...
  Upload TX: c8599bb1...
  Create TX: 1e934b4e...
  Status: SUCCESS
```

### Multi-contract dependency graphs

A `.sdkt.toml` workspace declares contracts under `[contracts.<alias>]`. Each
contract may declare dependencies using either the canonical `depends_on`
or the legacy `deploy_after` field — both are merged during
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

### Local package manifests

`sdkt package validate` lays the groundwork for a future package registry
**without** introducing any network or remote-registry functionality. It checks
a local package manifest declared in `.sdkt.toml`:

```toml
[package]
name = "my-token"
version = "0.1.0"
description = "Example Soroban token"

[dependencies.math]
path = "../math"

[dependencies.auth]
path = "./auth"
```

Validation rules (all offline — never performs network or registry I/O):

- `[package]` must have a `name` and a `version`.
- `version` must be a valid `MAJOR.MINOR.PATCH` shape (with optional
  pre-release/build metadata), matching the common semver form.
- Each `[dependencies.<name>]` entry is **exactly one source**: a local `path`
  **or** a `git` URL (never both). `deny_unknown_fields` rejects unrecognized
  keys so future source kinds (e.g. a registry) are explicit additions.
- For `git` deps, exactly one of `tag` / `branch` / `rev` must be set, and the
  URL must use `https`/`http`/`git`/`ssh` (or the `git@host:org/repo` SCP form).
- No self-dependency (a dependency key equal to the package `name`).
- For `path` deps, the referenced directory must exist.
- The dependency graph is checked for cycles (reusing the same topological-sort
  core as contract deploy-order resolution).

`sdkt package validate` exits non-zero on the first failure and prints a clear
message; with `--format json` it emits `{"valid": true}` or
`{"valid": false, "error": "..."}`.

### Fetching dependencies

`sdkt package fetch` materializes declared dependencies into a deterministic
local cache at `.sdkt-cache` (no registry, no authentication helpers, never
builds automatically):

```toml
[dependencies]
math = { path = "../math" }

token = { git = "https://github.com/org/token", tag = "v1.2.0" }
access = { git = "https://github.com/org/access", rev = "<commit>" }
utils = { git = "https://github.com/org/utils", branch = "main" }
```

- `path` deps are passed through (validated already by `validate`).
- `git` deps are cloned/checked out via the system `git` CLI into
  `.sdkt-cache/git/<stable-key>/`, then pinned to the requested `tag` /
  `branch` / `rev`. Re-running reuses an existing checkout unless `--force` is
  passed. A future registry source plugs into the same `DependencyFetcher`
  abstraction without touching callers.

The lock file (`sdkt.lock`) records each dependency's source, git URL,
requested reference, and resolved commit SHA (when available), so fetches are
reproducible. Local path deps remain unchanged in the lock.

| `sdkt deploy --wasm <file> [--salt <salt>]` | Upload WASM + instantiate. Salt is auto-generated if omitted (see [Deploy a single contract](#deploy-a-single-contract) below). Add `--deny-breaking --old-wasm <deployed.wasm>` to abort on a non-backwards-compatible upgrade. |
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
excluded.) See [docs/cli.md](docs/reference/cli.md) and [docs/examples.md](docs/getting-started/examples.md).

Most commands accept `--format json` for scripting / CI integration.

## Common Workflows

- **Audit every PR** — gate merges on `sdkt audit` (fails on `critical`). See
  [docs/ci-cd.md](docs/compatibility/ci-cd.md).
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
[docs/examples.md](docs/getting-started/examples.md).

## Upgrade Safety in CI

`sdkt` ships a reusable GitHub composite Action. See
[docs/ci-cd.md](docs/compatibility/ci-cd.md) for copy-paste workflows (audit-on-PR,
upgrade-safety-on-release).

## Documentation

- [docs/quick-start.md](docs/getting-started/quick-start.md) — five-minute first-time walkthrough.
- [docs/getting-started.md](docs/getting-started/getting-started.md) — deeper offline `diff` and `audit` examples.
- [docs/examples.md](docs/getting-started/examples.md) — command recipes & CI gating.
- [docs/installation.md](docs/getting-started/installation.md) — build / install / features.
- [docs/compatibility.md](docs/compatibility/compatibility.md) — real-world contract compatibility matrix.
- [docs/ci-cd.md](docs/compatibility/ci-cd.md) — CI/CD with the reusable Action.
- [docs/adoption.md](docs/advanced/adoption.md) — ecosystem adoption and integration evidence.
- [SECURITY.md](SECURITY.md) — supported versions and vulnerability reporting.
- [CONTRIBUTING.md](CONTRIBUTING.md) — how to contribute.

Additional references: [docs/cli.md](docs/reference/cli.md) (full command reference),
[docs/faq.md](docs/getting-started/faq.md) (FAQ),
[docs/plugin-authoring.md](docs/plugins/plugin-authoring.md) (write your own audit
rules), and [ROADMAP.md](ROADMAP.md) · [CHANGELOG.md](CHANGELOG.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All contributions are welcome — docs,
tests, and small fixes are great first PRs. Please follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

[MIT](LICENSE).
