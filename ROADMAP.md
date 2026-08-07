# Soroban DevKit (`sdkt`) — Roadmap

**Last updated:** 2026-08-07
**Status:** Active development · default branch `main` · current release **v2.2.0**

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
| **Current release** | `v2.2.0` (tags `v2.0.0`, `v2.1.0`, `v2.1.1` also published) |
| **Repository status** | Active · all milestones through **M27** merged to `main` |
| **Crates** | 8 (`sdkt-cli` + 7 supporting crates) |
| **Completed milestones** | 25 (M3A, M3B, M5–M26, M27) |
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
`v1.0.0` → `v2.2.0` release line.

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

### Release Engineering

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

### RPC & Simulation

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M8 | Mutability foundation | `sdkt tx simulate`, `sdkt tx submit`, `sdkt identity` (ED25519 keystore), `sdkt tx build` envelope builder, fee estimation | v0.8.0-alpha |
| M25 | RPC Connection Pooling (ENG-01) | Persistent pooled `reqwest::Client` in `SorobanRpcClient`; configurable `timeout_secs` / `pool_max_idle_per_host` | main |
| M26 | Transaction Simulation Enhancements (ENG-03) | `sdkt tx simulate` surfaces `restorePreamble` and granular `stateChanges` | main |
| M27 | Native Transaction Signing | `sdkt tx sign` signs envelopes with a local ED25519 identity (offline); `sdkt-xdr` signing library (`sign_transaction`, `Ed25519Signer`, `Network`, `Signer`); `sdkt-storage::IdentityStore::load_signing_key` keystore integration | main |

---

## 5. Current Status

**Where is this project today?**

- **Completed milestones:** 25 — M3A, M3B, M5, M6, M7, M8, M9, M10, M11, M12, M13, M14, M15, M16, M17, M18, M19, M20, M21, M22, M23, M24, M25, M26, M27.
- **Active milestone:** None in progress. The latest merged work is M27 (native transaction signing, shipped in `v2.2.0`); mainline development now tracks the Post-2.0 direction in §6.
- **Current release:** `v2.2.0` (tagged). Prior tagged releases: `v2.1.1`, `v2.1.0`, `v2.0.0`.
- **Repository health:** Healthy. 8 crates, all quality gates enforced in CI (`cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` default + all-features, `cargo test --workspace`).
- **CI status:** Green. Workflows: `ci.yml` (fmt/clippy/test on Ubuntu/macOS/Windows + MSRV + install-script validation), `release.yml` (tag-gated cross-platform binaries, checksums, crates.io publish), `compatibility.yml` (real-world `stellar/soroban-examples` validation), `sdkt-action-ci.yml` (self-validates the reusable Action).

---

## 6. Next Priorities

No new milestones are defined here — the items below reflect the existing
Post-2.0 direction already recorded in `RELEASE_READINESS.md` and
`CHANGELOG.md`. They are tracked as backlog, not yet scheduled.

### High Priority

- **Mainnet-focused tooling** — capabilities oriented toward production mainnet
  usage (Post-2.0 theme).
- **Containerized distribution** — a Docker image for reproducible, portable
  runs (listed as planned in `RELEASE_READINESS.md`).

### Future Work

- **Plugin ecosystem** — tooling and conventions for sharing/consuming
  third-party audit rules (native + WASM) built on M17–M19.
- **SCF grant alignment** — positioning `sdkt` for Stellar Community Fund
  grant tracks (Post-2.0 theme).
- **Developer productivity** — continuing the DX investments started in M7/M20
  (faster feedback, better errors, smoother onboarding).

### Long-Term Vision

- **Plugin marketplace** — a managed catalog of community audit rules,
  extending the M17–M19 plugin foundation into a shared ecosystem.
- **Broader Soroban ecosystem integration** — deeper compatibility and
  first-class support for the contracts developers actually deploy.

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
