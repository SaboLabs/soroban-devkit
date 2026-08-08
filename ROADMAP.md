# Soroban DevKit (`sdkt`) — Roadmap

**Last updated:** 2026-08-07
**Status:** Active development · default branch `main` · current release **v2.5.0**

This document is the single source of truth for milestone scope and sequencing.
Individual milestone plans live under `docs/milestone-*-plan.md`; engineering
design tickets live under `.project/issues/<version>/`.

---

## 1. Executive Summary

`sdkt` is a unified, offline-capable CLI and Rust toolkit for Stellar / Soroban
development — it consolidates the fragmented contract lifecycle (decode,
inspect, analyze, build, simulate, submit, audit, deploy) into one
production-grade binary.

| | |
|---|---|
| **Current release** | `v2.5.0` (tags `v2.0.0`, `v2.1.0`, `v2.1.1`, `v2.2.0`, `v2.3.0`, `v2.4.0`, `v2.5.0` also published) |
| **Repository status** | Active · all milestones through **M40** merged to `main` |
| **Crates** | 8 (`sdkt-cli` + 7 supporting crates) |
| **Completed milestones** | 33 (M3A, M3B, M5–M29, M35.0, M35.1, M35.2, M36.0, M37, M38, M39, M40) |
| **Current focus** | Post-2.0 direction — mainnet tooling, plugin ecosystem, SCF alignment (see §6) |
| **Original gap analysis** | [`GAP_ANALYSIS.md`](GAP_ANALYSIS.md) — market-gap justification |

A new contributor should be able to understand the project from this summary
alone: a mature, test-covered Soroban toolchain with a clear path toward
mainnet readiness and an extensible plugin architecture.

---

## 2. Vision

`sdkt` unifies the Soroban developer lifecycle — inspect, decode, analyze,
build, simulate, and submit — into one modular, offline-capable Rust toolkit,
instead of juggling 5+ separate CLIs. See [`GAP_ANALYSIS.md`](GAP_ANALYSIS.md)
for the original market-gap justification.

---

## 3. Current Architecture

The workspace is a Cargo virtual workspace. The `sdkt` binary is produced by
`sdkt-cli`; all logic lives in focused, dependency-bounded crates.

| Crate | Purpose | Key Responsibilities |
|-------|---------|----------------------|
| `sdkt-core` | Global configuration & shared types | `DevKitConfig`, `NetworkConfig`, `OutputFormat`, `ValidationError`. No I/O, no networking. |
| `sdkt-xdr` | XDR decode / encode & payload manipulation | `decode()`, `encode_ledger_key()`, `extract_wasm_hash()`, `decode_event_topics()`, typed builder helpers. No networking, no I/O. |
| `sdkt-wasm` | Contract WASM inspection & offline analysis | `ContractSpec` parser, `WasmModule` inspector, `SpecDiff`, `UpgradeVerdict`. Offline only. |
| `sdkt-rpc` | Soroban RPC client & on-chain aggregation | `SorobanRpcClient` (persistent pooled `reqwest`), `TtlInfo`, `ContractInspection`; `simulate` / `submission` / `builder` modules. **The only network-I/O crate.** |
| `sdkt-storage` | Storage analysis, WASM caching, keystore | `StorageAnalyzer`, `StorageReport`, `WasmCache`, `IdentityStore` (ED25519, `~/.sdkt/identities`). |
| `sdkt-audit` | Offline static security analysis | `Severity`, `Finding`, `AuditReport`, `AuditRule`, `RuleRegistry`, `register_rule!`; built-in rules `AUTH-001/002/003`, `MOVE-001`; plugin author API. |
| `sdkt-audit-example-rule` | Reference plugin crate | Rule `EXAMPLE-001`; produces `libsdkt_audit_example_rule` (native) and `sdkt_audit_example_rule.wasm` behind the `plugins` / `wasm-plugins` features. |
| `sdkt-cli` | User-facing CLI | `Cli`, `Commands`; routes arguments to crates and formats output (pretty + `--format json`). Builds the `sdkt` binary. |

**Dependency rules**

- `sdkt-core` depends on no other workspace crate and performs no networking.
- `sdkt-xdr` and `sdkt-wasm` are offline / networking-free.
- `sdkt-rpc` is the sole network boundary (besides `sdkt-storage`'s keystore disk writes).
- Everything may depend on `sdkt-core`; `sdkt-cli` orchestrates the rest.

---

## 4. Development Progress

Milestones are grouped by theme. Numbering is unchanged; all historical scope
is preserved. Milestones **M16–M27** were merged to `main` across the
`v1.0.0` → `v2.4.0` release line.

### Foundation

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M3A / M3B | Storage & Inspect foundation | `sdkt storage check`, `sdkt inspect`, `sdkt-rpc` crate, `OutputFormat` canonicalized in `sdkt-core` | v0.4.0-alpha |
| M5 | Network introspection | `sdkt tx inspect`, `sdkt events`, `sdkt account`, generic RPC `request()` | v0.5.0-alpha |
| M6 | Production hardening | RPC retry/timeout, clippy strict, GitHub Actions CI, docs/rustdoc coverage | v0.6.0-alpha |

### Developer Experience

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M7 | Horizon account enrichment + ScVal pretty UI | Account graph via Horizon REST; human-readable ScVal pretty printing in CLI | v0.7.0-alpha |

### Storage & Inspection

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M10 | ABI-aware decoding (ENG-16) | `--abi <WASM>` on `events`/`inspect`/`storage check`; `decode_event_topics`; real event payload decoding | v0.10.0-alpha |
| M11 | StorageAnalyzer completion (Proposal B) | Finish `StorageAnalyzer` + `sdkt storage analyze` CLI | v0.11.0-alpha |
| M21 | Contract Inspector (Offline) | `sdkt wasm inspect <file.wasm>` — metadata, custom sections, exports, specs offline | main |
| M22 | Contract Verification | `sdkt verify` — confirms a deployed contract's on-chain WASM hash matches a local artifact | main |
| M23 | Contract Health Report | `sdkt health` — aggregates WASM hash + storage/TTL posture into a `healthy`/`at_risk`/`critical` verdict | main |

### Security & Analysis

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M12 | Contract ABI/WASM Diff (Candidate C) | `sdkt diff --old-wasm --new-wasm` offline comparison | v0.12.0-alpha |
| M13 | Gap C — Static Security Analysis (`sdkt audit`) | New `sdkt-audit` crate; `AUTH-001/002/003`, `MOVE-001`; `sdkt audit <path>` | v0.13.0-alpha |
| M14 | Upgrade Safety Guard (Candidate A) | `UpgradeVerdict`; `sdkt diff --upgrade-safety`; `sdkt deploy --deny-breaking` | v0.14.0-alpha |

### Plugin System

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M17 | Plugin System — Phase A (Rule Registry) | `RuleRegistry` in `sdkt-audit`; additive `--rules <path>`; plugin author API; example rule crate; `docs/plugin-authoring.md` | main |
| M18 | Plugin System — Phase B (Dynamic Rule Loading) | Native `.so`/`.dylib`/`.dll` plugins via `libloading` + C-ABI; `sdkt audit --rules <plugin.so>`; ABI major-version gate (feature `plugins`, default OFF) | main |
| M19 | Plugin System — Phase C (WASM Sandbox) | Sandboxed `.wasm` plugins via `extism` + JSON-ABI; `sdkt audit --rules <plugin.wasm>`; no FS/network (feature `wasm-plugins`, default OFF) | main |
| M40 | Plugin Ecosystem (Local Store & Distribution) | Local offline plugin store (`plugin.toml` metadata), `sdkt plugin list/show/install/remove/update` (local-only), identity-based `--rules <id>` resolution; reuses M17–M19 loaders; NO hosted registry | main (merged in v2.5.0) |

### Soroban Ecosystem Integration

| Milestone | Theme | Highlights | Release |
|---|---|---|---|
| M41 | On-Chain Contract Interface & Instance Inspection | Wire existing `get_wasm_metadata` into `inspect_contract`; enrich `ContractInspection` with on-chain WASM size, parsed ABI (functions/events/types), storage summary, TTL, storage keys; `sdkt wasm metadata --contract <id>` returns a complete report; add network-guarded on-chain compatibility coverage | main (merged in v2.5.0) |
| M42 | On-Chain Upgrade-Safety Verification | Bridge M41 (deployed-WASM retrieval) with M14 (`SpecDiff`/`UpgradeVerdict`): `sdkt verify --contract <id> --wasm <candidate.wasm> --upgrade-safety` fetches the live contract's `ContractSpec` and classifies breaking vs non-breaking changes vs a local candidate. Reuses `inspect_contract`/`get_wasm_bytecode`, `parse_contract_spec`, existing `SpecDiff`/`UpgradeVerdict`; no new RPC method, no new engine. Plan: `docs/milestone-42-plan.md` | main (merged in v2.5.0) |
| M43 | Live-Contract ABI for Events Decode | Extend `sdkt events` with `--abi-contract <id>` so a deployed contract's on-chain WASM (fetched via M41 `inspect_contract`/`get_wasm_bytecode`, parsed by `parse_contract_spec`) supplies the ABI for `decode_event_topics` (M10). No local WASM artifact required. Reuses M41 retrieval + M10 decoding; no new RPC method, no new parser. Plan: `docs/milestone-43-plan.md` | main (merged in v2.5.0) |
| M44 | On-Chain ABI for Storage Decode | Extend `sdkt storage` with `--abi-contract <id>` so a deployed contract's on-chain WASM (M41 path) supplies the ABI for the existing storage analyzer's ABI-aware decode, mirroring M43. `--abi <path>` unchanged; mutually exclusive. Reuses M41 retrieval + existing storage analyzer; no new RPC method, no new decoder. Plan: `docs/milestone-44-plan.md` | main (implemented on `feat/milestone-44`, pending merge) |

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M15 | CI/CD GitHub Action (`sdkt` composite Action) | `.github/actions/sdkt/action.yml` wraps `sdkt audit` + `sdkt diff --upgrade-safety`; `docs/ci-cd.md` + self-validating workflow | v0.15.0-alpha |
| M16 | Release Engineering & Polish | Unified workspace version (`0.16.0-alpha`); Action install fix; `release.yml`; README/`docs/cli.md` rewrite; panic audit on user paths | v0.16.0-alpha |
| M20 | Stability & Release Engineering | 1.88.0 MSRV bump, CI hardening, `sdkt-storage` Windows compatibility, dependency compaction | main |

### Workspace / Build

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M9 | WASM tooling & caching | `sdkt wasm metadata`, `sdkt wasm cache`, `sdkt-wasm` crate, ContractSpec parser, `sdkt deploy` + `sdkt init` scaffolding | v0.9.0-alpha |
| M24 | Workspace & Build Orchestration | `sdkt build` (compiles artifacts) and `sdkt project deploy` (topological sorting, `.sdkt.toml` workspaces) | main |

### Package Manager & Distribution

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M35.0 | Local package manifest foundation | `.sdkt.toml` `[package]` + `[dependencies]` (local `path` only); `sdkt package validate` offline | main |
| M35.1 | Git dependency sources | `git` deps (`tag`/`branch`/`rev`); `sdkt package fetch` into `.sdkt-cache`; `DependencyFetcher` trait | main |
| M35.2 | Lock dependency resolution & reproducible verification | `sdkt.lock` records resolved commit/integrity; `verify_dependencies` + `sdkt lock verify` cover deps | main |
| M36.0 | Package update & synchronization | `sdkt package update` (`--check`/`--dry-run`/`--format`); closes `validate → fetch → update → verify` | main |
| M37 | Dependency Version Resolution | Semver `version` constraints on deps; `VersionResolver` picks best satisfying tag/commit; `--check` reports constraint state | main (scheduled) |
| M38 | Packaging & Publishing Workflow | `sdkt package pack` (offline bundle of manifest+lock+cache); `sdkt package publish --dry-run` readiness check | main (scheduled) |
| M39 | Release Polish & SCF Readiness | `Dockerfile` distribution, mainnet-safety guards, SCF positioning doc, `RELEASE_READINESS.md` refresh, opt-in `--version` provenance | main (scheduled) |

### RPC & Simulation

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M8 | Mutability foundation | `sdkt tx simulate`, `sdkt tx submit`, `sdkt identity` (ED25519 keystore), `sdkt tx build` envelope builder, fee estimation | v0.8.0-alpha |
| M25 | RPC Connection Pooling (ENG-01) | Persistent pooled `reqwest::Client` in `SorobanRpcClient`; configurable `timeout_secs` / `pool_max_idle_per_host` | main |
| M26 | Transaction Simulation Enhancements (ENG-03) | `sdkt tx simulate` surfaces `restorePreamble` and granular `stateChanges` | main |
| M27 | Native Transaction Signing | `sdkt tx sign` signs envelopes with a local ED25519 identity (offline); `sdkt-xdr` signing library (`sign_transaction`, `Ed25519Signer`, `Network`, `Signer`); `sdkt-storage::IdentityStore::load_signing_key` keystore integration | main |
| M28 | Network Profiles (Storage + CLI) | `sdkt-storage::NetworkStore` + `NetworkProfile` (M28.1); `sdkt network add/list/show/remove` CLI (M28.2) | main |
| M29 | Network Profile Integration | `--network-profile <NAME>` plus `--rpc-url` / `--network-passphrase` override flags on every RPC command; precedence flags > profile > `.sdkt.toml` > defaults | main |

---

## 5. Current Status

**Where is this project today?**

- **Completed milestones:** 33 — M3A, M3B, M5, M6, M7, M8, M9, M10, M11, M12, M13, M14, M15, M16, M17, M18, M19, M20, M21, M22, M23, M24, M25, M26, M27, M28, M29, M35.0, M35.1, M35.2, M36.0, M37, M38, M39, M40.
- **Active milestone:** **M44 (On-Chain ABI for Storage Decode)** is implemented on branch `feat/milestone-44` (pending merge); plan at `docs/milestone-44-plan.md`. It extends `sdkt storage` with `--abi-contract <id>` so a deployed contract's on-chain WASM (fetched via the M41 `inspect_contract`/`get_wasm_bytecode` path, parsed by `parse_contract_spec`) supplies the ABI for the existing storage analyzer's decode path — mirroring M43, no local WASM artifact required. Reuses M41 retrieval + existing storage analyzer; no new RPC method, no new decoder. **M43 (Live-Contract ABI for Events Decode) is merged and shipped in `v2.5.0`**; M42 (On-Chain Upgrade-Safety Verification), M41 (On-Chain Contract Interface & Instance Inspection), and M40 (Plugin Ecosystem — Local Store & Distribution) are also merged and shipped in `v2.5.0`.
- **Current release:** `v2.5.0` (tagged). Prior tagged releases: `v2.4.0`, `v2.3.0`, `v2.2.0`, `v2.1.1`, `v2.1.0`, `v2.0.0`.
- **Repository health:** Healthy. 8 crates, all quality gates enforced in CI (`cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` default + all-features, `cargo test --workspace`).
- **CI status:** Green. Workflows: `ci.yml` (fmt/clippy/test on Ubuntu/macOS/Windows + MSRV + install-script validation), `release.yml` (tag-gated cross-platform binaries, checksums, crates.io publish), `compatibility.yml` (real-world `stellar/soroban-examples` validation), `sdkt-action-ci.yml` (self-validates the reusable Action).

---

## 6. Next Priorities

The package-manager line is fully scheduled and completed through M39; M40 (plugin
local store), M41 (on-chain inspection), M42 (on-chain upgrade-safety), and M43
(Live-Contract ABI for Events Decode) are merged and shipped in `v2.5.0`. The next
scheduled milestone is **M44 (On-Chain ABI for Storage Decode)** — see
`docs/milestone-44-plan.md`. The remaining backlog items below are explicitly
unscheduled:

- **M44 — On-Chain ABI for Storage Decode.** Extend `sdkt storage` with
  `--abi-contract <id>` so a deployed contract's on-chain WASM (fetched via the M41
  `inspect_contract`/`get_wasm_bytecode` path, parsed by `parse_contract_spec`) supplies
  the ABI for the existing storage analyzer's decode path — mirroring M43, no local
  WASM artifact required. Plan: `docs/milestone-44-plan.md`.

### Future Work (unscheduled backlog)

The following remain tracked as backlog, not yet assigned milestone IDs:

- **Plugin ecosystem / marketplace — REMOTE slice.** M40 (scheduled) delivers the
  local, offline-first store + install/remove/list/update (local sources only).
  The remaining remote/marketplace layer — a hosted index/server, remote
  `https` plugin sources, `sdkt plugin update` from a remote, plugin signing /
  checksum verification, and a `.sdktplugin` bundle format — stays unscheduled
  backlog.
- **Broader Soroban ecosystem integration** — M41 (scheduled) covers the
  on-chain contract interface & instance inspection slice of this backlog item.
  Deeper compatibility-matrix work (beyond the on-chain inspection path) remains
  unscheduled.
- **Developer productivity** — continuing the DX investments started in M7/M20 (faster feedback, better
  errors, smoother onboarding).
- **Hosted package registry** — a remote index/server that the `DependencyFetcher` trait (M35.1) can
  target; explicitly deferred past M38.

---

## 7. Gap Closure Matrix

Status of the original gaps identified in `GAP_ANALYSIS.md`. Every row reflects
the actual repository state (all Plugin System phases are merged).

| Original Gap | State |
|--------------|-------|
| Gap A — Unified CLI lifecycle | ✅ Closed (M3A–M10) |
| Gap B — Storage rent visibility | ✅ Closed (M3A) |
| Gap C — Static security analysis | ✅ **Closed (M13)** — `sdkt-audit` crate, `AUTH-001/002/003` + `MOVE-001` rules, `sdkt audit` CLI |
| Gap D — Local XDR decoder | ✅ Closed (M3A/M5) |
| Gap E — ABI/interface viewer | ✅ Closed (M3B/M10) |
| Plugin system | ✅ **Closed (M17–M19)** — Phase A (`RuleRegistry` + plugin author API + example rule crate), Phase B (dynamic native `.so`/`.dylib`/`.dll` loading), and Phase C (sandboxed `.wasm` plugins via `extism`) are all merged to `main`. Extensibility pillar; built on the stable `AuditRule` trait from M13. |

---

## 8. Development Principles

1. **Read-only before mutating** — read-only features ship first (M3A–M7), mutating features follow (M8+).
2. **One surface per milestone** — each milestone adds one new crate or one major CLI surface, keeping compile times and review scope bounded.
3. **Mandatory quality gates** — `cargo fmt` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` are required for every PR.
4. **Branch discipline** — default branch is `main`; PRs target `main` from `feat/milestone-NN`.
