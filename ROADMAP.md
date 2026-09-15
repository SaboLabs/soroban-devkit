# Soroban DevKit (`sdkt`) — Roadmap

**Last updated:** 2026-08-07
**Status:** Active development · default branch `main` · current release **v2.5.0**

---

## 1. Executive Summary

`sdkt` is a unified, offline-capable CLI and Rust toolkit for Stellar / Soroban development — it consolidates the fragmented contract lifecycle (decode, inspect, analyze, build, simulate, submit, audit, deploy) into one binary.

| | |
|---|---|
| **Current release** | `v2.5.0` (tags `v2.0.0`, `v2.1.0`, `v2.1.1`, `v2.2.0`, `v2.3.0`, `v2.4.0`, `v2.5.0` also published) |
| **Repository status** | Active · all capabilities merged to `main` |
| **Crates** | 8 (`sdkt-cli` + 7 supporting crates) |
| **Current focus** | Post-2.0 direction — mainnet tooling, plugin ecosystem |

A new contributor can understand the project from this summary alone: a test-covered Soroban toolchain with a clear path toward mainnet readiness and an extensible plugin architecture.

---

## 2. Vision

`sdkt` unifies the Soroban developer lifecycle — inspect, decode, analyze, build, simulate, and submit — into one modular, offline-capable Rust toolkit, instead of juggling 5+ separate CLIs.

---

## 3. Current Architecture

The workspace is a Cargo virtual workspace. The `sdkt` binary is produced by `sdkt-cli`; all logic lives in focused, dependency-bounded crates.

| Crate | Purpose | Key Responsibilities |
|-------|---------|----------------------|
| `sdkt-core` | Global configuration & shared types | `DevKitConfig`, `NetworkConfig`, `OutputFormat`, `ValidationError`. No I/O, no networking. |
| `sdkt-xdr` | XDR decode / encode & payload manipulation | `decode()`, `encode_ledger_key()`, `extract_wasm_hash()`, `decode_event_topics()`, typed builder helpers. No networking, no I/O. |
| `sdkt-wasm` | Contract WASM inspection & offline analysis | `ContractSpec` parser, `WasmModule` inspector, `SpecDiff`, `UpgradeVerdict`. Offline only. |
| `sdkt-rpc` | Soroban RPC client & on-chain aggregation | `SorobanRpcClient` (persistent pooled `reqwest`), `TtlInfo`, `ContractInspection`; `simulate` / `submission` / `builder` modules. **The only network-I/O crate.** |
| `sdkt-storage` | Storage analysis, WASM caching, keystore | `StorageAnalyzer`, `StorageReport`, `WasmCache`, `IdentityStore` (ED25519, `~/.sdkt/identities`). |
| `sdkt-audit` | Offline static security analysis | `Severity`, `Finding`, `AuditReport`, `AuditRule`, `RuleRegistry`, `register_rule!`; built-in rules `AUTH-001/002/003/004`, `MOVE-001`; plugin author API. |
| `sdkt-audit-example-rule` | Reference plugin crate | Rule `EXAMPLE-001`; produces `libsdkt_audit_example_rule` (native) and `sdkt_audit_example_rule.wasm` behind the `plugins` / `wasm-plugins` features. |
| `sdkt-cli` | User-facing CLI | `Cli`, `Commands`; routes arguments to crates and formats output (pretty + `--format json`). Builds the `sdkt` binary. |

**Dependency rules**

- `sdkt-core` depends on no other workspace crate and performs no networking.
- `sdkt-xdr` and `sdkt-wasm` are offline / networking-free.
- `sdkt-rpc` is the sole network boundary (besides `sdkt-storage`'s keystore disk writes).
- Everything may depend on `sdkt-core`; `sdkt-cli` orchestrates the rest.

---

## 4. Development Progress

Capabilities are grouped by theme below.

### Foundation

- Storage & Inspect — `sdkt storage check`, `sdkt inspect`, RPC client, canonical `OutputFormat` (shipped in early alpha releases)
- Network introspection — `sdkt tx inspect`, `sdkt events`, `sdkt account`, generic RPC (alpha)
- Production hardening — RPC retry/timeout, clippy strict, CI, docs/rustdoc coverage (alpha)

### Developer Experience

- Horizon account enrichment + ScVal pretty UI — Account graph via Horizon REST; human-readable ScVal output (alpha)

### Storage & Inspection

- ABI-aware decoding — `--abi <WASM>` on `events`/`inspect`/`storage check`; real event payload decoding (alpha)
- StorageAnalyzer — `sdkt storage analyze` CLI with full classification (alpha)
- Offline contract inspection — `sdkt wasm inspect <file.wasm>` reads metadata, custom sections, exports, specs
- Contract verification — `sdkt verify` confirms a deployed contract's on-chain WASM hash matches a local artifact
- Contract health report — `sdkt health` aggregates WASM hash + storage/TTL posture into a `healthy`/`at_risk`/`critical` verdict

### Security & Analysis

- ABI/WASM diff — `sdkt diff --old-wasm --new-wasm` offline comparison (alpha)
- Static security analysis — `sdkt-audit` crate with rules `AUTH-001/002/003/004` and `MOVE-001`; exposed via `sdkt audit <path>` (alpha)
- Upgrade safety guard — `UpgradeVerdict`; `sdkt diff --upgrade-safety`; `sdkt deploy --deny-breaking` (alpha)

### Plugin System

- Rule registry — `RuleRegistry` in `sdkt-audit`; additive `--rules <path>`; plugin author API; example rule crate
- Dynamic rule loading — Native `.so`/`.dylib`/`.dll` plugins via `libloading` + C-ABI; `sdkt audit --rules <plugin.so>`; ABI major-version gate (feature `plugins`, default OFF)
- WASM sandbox — Sandboxed `.wasm` plugins via `extism` + JSON-ABI; `sdkt audit --rules <plugin.wasm>`; no FS/network (feature `wasm-plugins`, default OFF)
- Plugin local store — Offline plugin store with `plugin.toml` metadata; `sdkt plugin list/show/install/remove/update` (local-only); identity-based `--rules <id>` resolution; shipped in `v2.5.0`

### Soroban Ecosystem Integration

- On-chain contract inspection — `sdkt wasm metadata --contract <id>` returns complete WASM metadata, parsed ABI, storage summary, and TTL
- On-chain upgrade-safety verification — `sdkt verify --contract <id> --wasm <candidate.wasm> --upgrade-safety` fetches the live contract's spec and classifies breaking vs non-breaking changes
- Live-contract ABI for events — `sdkt events --abi-contract <id>` uses the deployed contract's on-chain WASM to decode events without a local artifact
- Live-contract ABI for storage — `sdkt storage --abi-contract <id>` uses the deployed contract's on-chain WASM for storage analysis

### CI & Release

- GitHub composite Action — `.github/actions/sdkt/action.yml` wraps `sdkt audit` + `sdkt diff --upgrade-safety` for CI
- Release engineering — Unified workspace version; `release.yml`; install-script validation; panic audit on user paths
- Stability — MSRV gate, CI hardening, Windows compatibility, dependency compaction

### Workspace & Build

- WASM tooling & caching — `sdkt wasm metadata`, `sdkt wasm cache`, `sdkt-wasm` crate, `ContractSpec` parser, `sdkt deploy` + `sdkt init` scaffolding
- Workspace orchestration — `sdkt build` compiles artifacts; `sdkt project deploy` handles topological dependency sorting via `.sdkt.toml`

### Package Manager & Distribution

- Local package manifest — `.sdkt.toml` `[package]` + `[dependencies]` (local `path`); `sdkt package validate` offline
- Git dependency sources — `git` deps with `tag`/`branch`/`rev`; `sdkt package fetch` into `.sdkt-cache`
- Lock & reproducibility — `sdkt.lock` records resolved commit/integrity; `sdkt lock verify` covers deps
- Package update — `sdkt package update` with `--check`/`--dry-run`/`--format`; closes the validate → fetch → update → verify loop
- Version resolution — Semver constraints on deps; `VersionResolver` picks best satisfying tag/commit
- Packaging — `sdkt package pack` produces an offline bundle of manifest + lock + cache; `sdkt package publish --dry-run` readiness check
- Release polish — `Dockerfile` distribution, mainnet-safety guards, opt-in `--version` provenance

### RPC & Simulation

- Mutability foundation — `sdkt tx simulate`, `sdkt tx submit`, `sdkt identity` (ED25519 keystore), `sdkt tx build` envelope builder, fee estimation
- RPC pooling — Persistent pooled `reqwest::Client`; configurable timeout / pool settings
- Simulation detail — `sdkt tx simulate` surfaces `restorePreamble` and granular `stateChanges`
- Native transaction signing — `sdkt tx sign` signs envelopes with a local ED25519 identity (offline); `sdkt-xdr` signing library; keystore integration
- Network profiles — Stored RPC profiles with `sdkt network add/list/show/remove` CLI; `--network-profile <NAME>` plus override flags on every RPC command
- Profile integration — Precedence: explicit flags > profile > `.sdkt.toml` > built-in testnet default

---

## 5. Current Status

**Where is this project today?**

- **Released:** All capabilities are merged to `main` and shipped in releases through `v2.5.0`.
- **Current release:** `v2.5.0` (tagged). Prior tagged releases: `v2.4.0`, `v2.3.0`, `v2.2.0`, `v2.1.1`, `v2.1.0`, `v2.0.0`.
- **Repository health:** Healthy. 8 crates, all quality gates enforced in CI (`cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` default + all-features, `cargo test --workspace`).
- **CI status:** Green. Workflows: `ci.yml` (fmt/clippy/test on Ubuntu/macOS/Windows + MSRV + install-script validation), `release.yml` (tag-gated cross-platform binaries, checksums, crates.io publish), `compatibility.yml` (real-world `stellar/soroban-examples` validation), `sdkt-action-ci.yml` (self-validates the reusable Action).

---

## 6. Next Priorities

The package-manager, plugin-ecosystem, and on-chain inspection lines are fully shipped in `v2.5.0`. The remaining backlog items below are explicitly unscheduled:

### Future Work (unscheduled backlog)

- **Plugin ecosystem / marketplace — remote slice.** The local offline-first store is shipped. The remaining remote/marketplace layer — a hosted index/server, remote `https` plugin sources, remote `sdkt plugin update`, plugin signing / checksum verification, and a `.sdktplugin` bundle format — stays unscheduled backlog.
- **Broader Soroban ecosystem integration.** On-chain inspection is shipped. Deeper compatibility-matrix work (beyond the on-chain inspection path) remains unscheduled.
- **Developer productivity** — continuing DX investments (faster feedback, better errors, smoother onboarding).
- **Hosted package registry** — a remote index/server that the `DependencyFetcher` trait can target; explicitly deferred.

---

## 7. Gap Closure Matrix

Status of the original gaps identified in the project's inception. Every row reflects the actual repository state.

| Original Gap | State |
|--------------|-------|
| Gap A — Unified CLI lifecycle | ✅ Closed |
| Gap B — Storage rent visibility | ✅ Closed |
| Gap C — Static security analysis | ✅ Closed — `sdkt-audit` crate with rules `AUTH-001/002/003/004` + `MOVE-001`, `sdkt audit` CLI |
| Gap D — Local XDR decoder | ✅ Closed |
| Gap E — ABI/interface viewer | ✅ Closed |
| Plugin system | ✅ Closed — rule registry, dynamic native loading, and sandboxed WASM plugins are all merged to `main` |

---

## 8. Development Principles

1. **Read-only before mutating** — read-only features ship first, mutating features follow.
2. **Mandatory quality gates** — `cargo fmt` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` are required for every PR.
3. **Branch discipline** — default branch is `main`; PRs target `main` from feature branches.
