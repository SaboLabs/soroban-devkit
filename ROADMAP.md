# Soroban DevKit (sdkt) — ROADMAP

**Last updated:** 2026-08-05
**Status:** Active development. `main` is the default branch. Latest merged work: Milestone 15 (CI/CD GitHub Action, tagged `v0.15.0-alpha`). M16 (Release Engineering & Polish) implemented on `feat/milestone-16`, pending merge.

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
| **M13** | **Gap C — Static Security Analysis (`sdkt audit`)** | New `sdkt-audit` crate; `AUTH-001/002/003`, `MOVE-001`; `sdkt audit <path>`. ✅ Merged to `main` (v0.13.0-alpha, #11). | — |
| **M14** | **Upgrade Safety Guard (Candidate A)** | Reuse M12 `SpecDiff`: `UpgradeVerdict`; `sdkt diff --upgrade-safety`; optional `sdkt deploy --deny-breaking`. ✅ **Closed & tagged `v0.14.0-alpha`** (commit `dc31767`). | — |
| **M15** | **CI/CD GitHub Action (`sdkt` composite Action)** | `.github/actions/sdkt/action.yml` wraps `sdkt audit` + `sdkt diff --upgrade-safety` for CI; `docs/ci-cd.md` + self-validating workflow. ✅ **Closed & tagged `v0.15.0-alpha`**. | — |
| **M16** | **Release Engineering & Polish** | Unified workspace version (`0.16.0-alpha`); Action install fix; `release.yml` (binaries + `cargo publish`); README/`docs/cli.md` rewrite; panic audit on user paths. ✅ Implemented (commit on `feat/milestone-16`, pending merge). | — |
| **M17** | **Plugin System — Phase A (Rule Registry)** | `RuleRegistry` in `sdkt-audit`; built-ins register via registry; additive `--rules <path>` flag; plugin author API (`AuditRule`/`AuditContext`/`Finding`/`register_rule!`); example rule crate `sdkt-audit-example-rule`; `docs/plugin-authoring.md`. ✅ Implemented on `feat/milestone-17`, pending merge. | — |

### Remaining Roadmap

| Milestone | Theme | Status | Dependencies |
|-----------|-------|--------|--------------|
| M16 | Release Engineering & Polish | **Done** (workspace version unify, Action fix, release workflow, docs rewrite, panic audit) | — |
| M17 | Plugin System — Phase A (Rule Registry) | **Done** (`RuleRegistry`, `--rules` flag, plugin author API, example rule crate, `docs/plugin-authoring.md`) | — |
| Post-1.0 | Mainnet-focused tooling, SCF grant alignment, plugin system Phase B (dynamic loading), plugin registry/marketplace | Backlog | M17 (Phase A) |

---

## Gap Closure Matrix (vs GAP_ANALYSIS.md)

| Original Gap | State |
|--------------|-------|
| Gap A — Unified CLI lifecycle | ✅ Closed (M3A–M10) |
| Gap B — Storage rent visibility | ✅ Closed (M3A) |
| Gap C — Static security analysis | ✅ **Closed (M13)** | M13: `sdkt-audit` crate, `AUTH-001/002/003` + `MOVE-001` rules, `sdkt audit` CLI |
| Gap D — Local XDR decoder | ✅ Closed (M3A/M5) |
| Gap E — ABI/interface viewer | ✅ Closed (M3B/M10) |
| Plugin system | 🟢 Phase A done (M17) — `RuleRegistry` + plugin author API + example rule crate; Phase B (dynamic loading) planned post-1.0 | Extensibility pillar; depends on a stable `AuditRule` trait (provided by `sdkt-audit` in M13) |

---

## Sequencing Principles

1. Read-only features ship before mutating ones (honored: M3A–M7 read-only, M8+ mutating).
2. Each milestone = one new crate or one major CLI surface, keeping compile times and review scope bounded.
3. `cargo fmt` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` are mandatory gates for every PR.
4. Default branch is `main`; PRs target `main` from `feat/milestone-NN`.
