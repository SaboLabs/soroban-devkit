# Soroban DevKit (sdkt) — ROADMAP

**Last updated:** 2026-08-05
**Status:** Active development. `main` is the default branch. Latest merged work: Milestone 12 (Contract ABI/WASM Diff). M13 (Gap C static analysis) implemented on `feat/milestone-13`, pending merge.

This document is the single source of truth for milestone scope and sequencing.
Individual milestone plans live under `docs/milestone-*-plan.md`; engineering
design tickets live under `.hermes/issues/<version>/`.

---

## Vision

`sdkt` is a modular, offline-capable Rust toolkit that unifies the Soroban
developer lifecycle: inspect, decode, analyze, build, simulate, and submit —
without fragmenting across 5+ separate CLIs. See `GAP_ANALYSIS.md` for the
original market gap justification.

---

## Crate Layout (current)

```
sdkt-cli      → user-facing CLI (clap + tokio), routes to crates, formats output
sdkt-core     → DevKitConfig, NetworkConfig, OutputFormat (no I/O, no networking)
sdkt-xdr      → XDR decode/encode, ScVal <-> Rust, ABI-aware decoding
sdkt-rpc      → Soroban RPC client, storage/inspect/tx/events/account/sim/submit
sdkt-storage  → WASM cache, identity/keystore, storage analysis (StorageAnalyzer)
sdkt-wasm     → ContractSpec parser, ABI type lookup, WASM metadata
```

Dependency rule: `sdkt-core` depends on nothing internal; everything else may
depend on `sdkt-core` + `sdkt-xdr`. No networking in `sdkt-xdr`/`sdkt-core`.

---

## Milestone Status

### Completed

| Milestone | Theme | Highlights | Release |
|-----------|-------|-----------|---------|
| M3A / M3B | Storage & Inspect foundation | `sdkt storage check`, `sdkt inspect`, `sdkt-rpc` crate, `OutputFormat` canonicalized in `sdkt-core` | v0.4.0-alpha |
| M5 | Network introspection | `sdkt tx inspect`, `sdkt events`, `sdkt account`, generic RPC `request()` | v0.5.0-alpha |
| M6 | Production hardening | RPC retry/timeout, clippy strict, GitHub Actions CI, docs/rustdoc coverage | v0.6.0-alpha |
| M7 | Horizon account enrichment + ScVal pretty UI | Account graph via Horizon REST; human-readable ScVal pretty printing in CLI | v0.7.0-alpha |
| M8 | Mutability foundation | `sdkt tx simulate`, `sdkt tx submit`, `sdkt identity` (ED25519 keystore), `sdkt tx build` envelope builder, fee estimation | v0.8.0-alpha |
| M9 | WASM tooling & caching | `sdkt wasm metadata`, `sdkt wasm cache`, `sdkt-wasm` crate, ContractSpec parser, deploy (`sdkt deploy`) + init (`sdkt init`) scaffolding | v0.9.0-alpha |
| M10 | ABI-aware decoding (ENG-16) | `--abi <WASM>` on `events`/`inspect`/`storage check`; `decode_event_topics`; real event payload decoding | v0.10.0-alpha |
| M11 | StorageAnalyzer completion (Proposal B) | Finish `StorageAnalyzer` + `sdkt storage analyze` CLI. ✅ Merged to `main` (v0.11.0-alpha). | — |
| **M12** | **Contract ABI/WASM Diff (Candidate C)** | `sdkt diff --old-wasm --new-wasm` offline comparison. ✅ Merged to `main` (v0.12.0-alpha, #10). | — |
| **M13** | **Gap C — Static Security Analysis (`sdkt audit`)** | New `sdkt-audit` crate; `AUTH-001/002/003`, `MOVE-001`; `sdkt audit <path>`. ✅ Implemented (commit on `feat/milestone-13`, pending merge). | — |

### Remaining Roadmap

| Milestone | Theme | Status | Dependencies |
|-----------|-------|--------|--------------|
| M13 | Gap C — Static security analysis (`sdkt audit`) | **Done** (new `sdkt-audit` crate, M13) | — |
| M14 | Contract diff / upgrade safety analysis | Planned | M10 ABI, M12 diff |
| M15 | CI/CD GitHub Action packaging (`sdkt` as a composite action) | Planned | M11 (audit in CI) |
| Post-1.0 | Mainnet-focused tooling, SCF grant alignment | Backlog | — |

---

## Gap Closure Matrix (vs GAP_ANALYSIS.md)

| Original Gap | State |
|--------------|-------|
| Gap A — Unified CLI lifecycle | ✅ Closed (M3A–M10) |
| Gap B — Storage rent visibility | ✅ Closed (M3A) |
| Gap C — Static security analysis | ✅ **Closed (M13)** | M13: `sdkt-audit` crate, `AUTH-001/002/003` + `MOVE-001` rules, `sdkt audit` CLI |
| Gap D — Local XDR decoder | ✅ Closed (M3A/M5) |
| Gap E — ABI/interface viewer | ✅ Closed (M3B/M10) |
| Plugin system | 🟡 Planned (post-M13) | Extensibility pillar; depends on a stable `AuditRule` trait (now provided by `sdkt-audit` in M13) |

---

## Sequencing Principles

1. Read-only features ship before mutating ones (honored: M3A–M7 read-only, M8+ mutating).
2. Each milestone = one new crate or one major CLI surface, keeping compile times and review scope bounded.
3. `cargo fmt` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` are mandatory gates for every PR.
4. Default branch is `main`; PRs target `main` from `feat/milestone-NN`.
