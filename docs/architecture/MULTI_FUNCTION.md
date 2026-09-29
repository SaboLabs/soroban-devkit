# SDKT Multi-Function Architecture & Roadmap v1

**Baseline:** `900a70f75125a59f919d8e5bebf3e893ebb2888b`  
**Status:** Architecture direction, not a feature-complete implementation plan

## 1. Purpose

Soroban DevKit (`sdkt`) is evolving from a collection of useful CLI capabilities into an integrated developer, security, inspection, and operations toolkit for Soroban.

The goal is not to maximize the number of commands. The goal is to make existing and future capabilities share stable foundations, predictable output, and clear boundaries.

## 2. Product Direction

```
                    SOROBAN DEVKIT
                         |
       +-----------------+-----------------+
       |                 |                 |
    DEVELOP            ANALYZE           OPERATE
       |                 |                 |
   project             audit             deploy
   build               ABI               invoke
   test                WASM              tx
   package             storage           network
   client-gen          diff              verify
   XDR                 diagnostics       inspect
       |                 |                 |
       +-----------------+-----------------+
                         |
                 SHARED CORE ENGINES
                         |
              +----------+----------+
              |          |          |
             CLI       Plugins   Playground
```

## 3. Current Capability Map

### Develop
- project initialization and build
- project deployment and dependency ordering
- package validation/fetch/update/pack/publish readiness
- lockfile generation and verification
- client generation
- XDR construction and manipulation

### Inspect
- WASM metadata and contract inspection
- transaction inspection
- account, event, storage, and network inspection
- ABI-aware decoding
- storage analysis and TTL visibility
- WASM cache inspection

### Analyze
- WASM/ABI diff
- upgrade-safety analysis
- contract verification
- contract health reporting
- storage snapshots and diffs

### Security
- static audit engine
- built-in security rules
- native audit plugins
- sandboxed WASM audit plugins
- local plugin store
- SARIF-oriented audit output

### Operate
- deployment
- invocation
- transaction build/simulation/sign/submit
- fee estimation
- network profiles
- identity/keystore operations
- on-chain contract inspection

### Web
- browser-local WASM playground
- shared `sdkt-wasm` inspection engine

## 4. Architecture Principles

1. **Offline-first:** analysis and inspection should not require RPC unless the operation inherently needs live chain state.
2. **Single network boundary:** `sdkt-rpc` remains the network-I/O boundary.
3. **Shared engines:** CLI, plugins, and browser tooling should reuse library functionality rather than duplicate protocol logic.
4. **Structured output:** machine-readable JSON and stable exit behavior are first-class integration surfaces.
5. **Read-only before mutation:** inspection, analysis, and verification should remain separable from state-changing operations.
6. **Small bounded changes:** contributor work should target independently useful gaps rather than broad rewrites.
7. **No speculative features:** new roadmap items require a demonstrated current-state use case or architectural need.
8. **Compatibility over reinvention:** existing commands should not be renamed or regrouped solely for aesthetics.

## 5. Target Subsystems

### A. Workspace / Project

```
project
├── init
├── build
├── test
├── status
├── package
├── lock
└── deploy
```

The project layer should become the workspace-level abstraction for multi-contract projects.

### B. Contract Intelligence

```
contract intelligence
├── WASM
├── ABI
├── storage
├── events
├── diff
├── verification
├── health
└── audit
```

This layer should share parsers, metadata, and analysis primitives rather than introducing parallel implementations.

### C. Transaction / Network Operations

```
transaction
├── build
├── validate
├── decode
├── simulate
├── sign
├── submit
├── inspect
└── diagnostics
```

Live operations remain clearly separated from offline transaction construction and decoding.

### D. Extension Layer

The existing audit plugin architecture is the first extension mechanism. Future extensions should reuse stable APIs and explicit trust boundaries rather than bypassing the core architecture.

### E. Playground

The Playground should remain a browser-local presentation layer for capabilities that can safely run in WASM. It should reuse `sdkt-wasm` and related shared engines where practical.

## 6. Current Genuine Backlog Mapping

Existing issues already map naturally to the direction above:

| Area | Existing issue | Scope |
|---|---:|---|
| Project | #234 | inspect recorded project deployments |
| Deployment | #243 | per-contract constructor arguments |
| Storage | #176 | richer TTL context |
| WASM/cache | #167 | valid JSON for cache information |
| Diagnostics | #215 | health verdict threshold consistency |
| Security | #179 | CEI-001 audit rule |
| Transactions | #69 | transaction failure diagnostics |
| Transactions | #169 | fee-bump transaction support |

These are references to existing backlog items, not new feature commitments. Before assignment, each issue should be revalidated against current `main` and against open PRs to avoid duplication.

## 7. Architectural Work to Watch

The CLI currently carries substantial command-routing/orchestration responsibility. As the toolkit grows, command-module boundaries may need to become more explicit.

This is **not** a mandate for a large CLI rewrite.

Refactoring should happen only when:
- duplicated orchestration appears,
- a contributor-facing subsystem becomes difficult to isolate,
- testing becomes unnecessarily coupled,
- or a concrete feature cannot be implemented cleanly without better boundaries.

## 8. Roadmap Sequence

### Stage 1 — Foundation
Stabilize shared configuration, output, errors, network resolution, XDR, WASM, RPC, and audit boundaries.

### Stage 2 — Workspace
Complete genuine project/workspace gaps such as deployment inspection and multi-contract deployment ergonomics.

### Stage 3 — Contract Intelligence
Continue improving ABI, WASM, storage, diff, verification, health, and audit interoperability.

### Stage 4 — Operations
Improve transaction diagnostics, deployment workflows, live inspection, and verification while preserving explicit network safety.

### Stage 5 — Web
Expand the Playground only where browser-local execution provides a useful interface to existing SDKT capabilities.

### Stage 6 — Extensions
Expand plugin capabilities only after the core APIs and trust boundaries are stable.

## 9. Contributor Issue Policy

Every new contributor issue should satisfy all of these:

- addresses a real current-state gap;
- has a bounded implementation surface;
- can be tested independently;
- does not duplicate an open PR or existing issue;
- does not require an unrelated architecture rewrite;
- has a concrete acceptance condition;
- fits one of the SDKT subsystems above.

Do not create issues merely to increase activity.

## 10. What This Roadmap Does Not Commit To

This document does not commit SDKT to:
- a fixed command namespace such as `sdkt contract ...`;
- a remote plugin marketplace;
- a hosted package registry;
- a large GUI;
- replacing existing commands;
- a monolithic refactor;
- speculative network features.

Those decisions require separate evidence and design work.

## 11. Definition of Multi-Function SDKT

SDKT should be considered multi-function when a developer can use one coherent toolkit to move through the Soroban lifecycle:

```
project
  -> build
  -> inspect
  -> analyze
  -> audit
  -> simulate
  -> deploy
  -> invoke
  -> inspect live state
  -> verify / diagnose
```

while the underlying implementation remains modular, offline-first where possible, testable, and extensible.
