# SCF / Grant Readiness — Soroban DevKit (`sdkt`)

> Status: **Self-documented grant-readiness positioning (M39).** This document
> describes the project as it actually exists today. It makes **no claim** of
> grant approval, partnership, funding, or production-mainnet guarantee. It is
> intended as supporting material for a Stellar Community Fund (SCF) application
> and for technically evaluating `sdkt`.

## What `sdkt` is

`sdkt` is a unified, offline-capable command-line toolkit and Rust library for
Stellar / Soroban smart-contract development. It consolidates the fragmented
contract lifecycle — decode, inspect, analyze, build, simulate, submit, audit,
and deploy — into a single MIT-licensed binary and reusable crate set.

- **Repository:** https://github.com/SaboLabs/soroban-devkit
- **License:** MIT
- **Language / edition:** Rust 2021, MSRV pinned to `1.88.0`
- **Current version:** `2.5.0`
- **Crates:** 8 (`sdkt-cli` + 7 supporting crates)
- **CI:** GitHub Actions (fmt, clippy `-D warnings`, test on Ubuntu/macOS/Windows,
  MSRV gate, install-script validation, real-world `stellar/soroban-examples`
  compatibility workflow)

## Why it fits the SCF / Stellar ecosystem

1. **Directly serves Soroban builders.** The toolchain addresses day-to-day
   friction in the Soroban developer experience: reading on-chain state,
   decoding events, diffing contract upgrades, and static security analysis —
   all from one CLI instead of five or more separate tools.
2. **Offline-first and reproducible.** Most commands require no network. Builds
   are pinned to a single workspace version and an MSRV; the binary can be built
   bit-for-bit deterministically (no git/date embedded unless the opt-in
   `provenance` feature is enabled at release time). This aligns with the
   supply-chain and reproducibility expectations grant reviewers favor.
3. **MIT-licensed and extensible.** A plugin system across **M17–M19** provides the
   architecture and loaders (native `.so`/`.dylib`/`.dll` via `libloading`, and
   sandboxed `.wasm` via `extism`), letting the community author and share additional
   static-analysis rules. **M40** adds a local plugin store and management
   (`sdkt plugin list/show/install/remove/update`, all local-only; identity-based
   `--rules <id>` resolution). A hosted/remote plugin registry is **not** part of M40
   and remains an explicitly deferred backlog item.
4. **Security-minded by default.** Mutating commands (submit, deploy) ship with
   a conservative mainnet-safety guard (M39) that refuses to sign for mainnet
   unless the operator explicitly selects the network, protecting against the
   classic testnet-default-meets-mainnet-endpoint foot-gun.

## Capability matrix

| Capability | Command(s) | Network required? |
|---|---|---|
| XDR decode (base64 → JSON) | `sdkt decode` | No |
| Contract ABI + storage inspection | `sdkt inspect`, `sdkt storage check` | Read (RPC) |
| Event exploration (raw + ABI-decoded) | `sdkt events` | Read (RPC) |
| Storage TTL / rent visibility | `sdkt storage analyze` | Read (RPC) |
| WASM metadata & offline diff | `sdkt wasm metadata`, `sdkt diff` | No |
| On-chain WASM hash verification | `sdkt verify` | Read (RPC) |
| Contract health posture report | `sdkt health` | Read (RPC) |
| Static security analysis | `sdkt audit` (+ plugin rules) | No |
| Upgrade-safety verdict | `sdkt diff --upgrade-safety` | No |
| Transaction build / simulate / submit | `sdkt tx build/simulate/submit` | Simulate/Submit (RPC) |
| Offline transaction sign | `sdkt tx sign` | No |
| Contract deploy (single + workspace) | `sdkt deploy`, `sdkt project deploy` | Write (RPC) |
| Network profile management | `sdkt network add/list/show/remove` | No |
| Package manifest, lock, pack | `sdkt package *`, `sdkt lock *` | No (fetch optional) |
| Shell completions | `sdkt completions` | No |
| Containerized distribution | `Dockerfile` | No (runtime RPC) |

## Reproducible-build / offline evidence

- `cargo build --release --bin sdkt` produces a single static binary; no
  network is required at runtime for offline commands.
- The `provenance` feature is **disabled by default**. Without it, `sdkt
  --version` reports only the semantic version, so builds are reproducible.
- Continuous integration runs the same quality gates on three operating
  systems and validates the binary against real `stellar/soroban-examples`
  contracts.

## Current maturity

- 40+ milestones merged to `main` (storage/inspect foundation through M44 — the
  on-chain ABI for storage decode). Recent milestones specifically deepen Soroban
  integration:
  - **M40** — local plugin store & management (no hosted registry).
  - **M41** — on-chain contract interface & instance inspection (`sdkt wasm
    metadata --contract <id>`).
  - **M42** — on-chain-vs-local upgrade-safety verification (`sdkt verify
    --contract <id> --wasm <candidate> --upgrade-safety`).
  - **M43** — live-contract ABI for events decode (`sdkt events <id>
    --abi-contract <id>`).
  - **M44** — on-chain ABI for storage decode (`sdkt storage analyze <id>
    --abi-contract <id>`).
- All four mandatory quality gates (fmt, clippy `-D warnings`, workspace test
  suite, MSRV) are enforced on every pull request.
- The CLI command surface is stable; no command has been removed or renamed in
  its lifecycle.

## Roadmap alignment (honest)

What is **shipped**: the full decode/inspect/analyze/build/simulate/submit/audit/deploy
lifecycle, static analysis with a plugin system (M17–M19 architecture + M40 local
store), on-chain contract inspection (M41), on-chain-vs-local upgrade-safety
verification (M42), live-contract ABI for events decode (M43), and on-chain ABI for
storage decode (M44). Network profiles and an offline package/lock workflow are also
shipped.

What is **explicitly not yet implemented** (deferred backlog, not claimed as done):
a hosted package registry / remote plugin marketplace, deeper first-class support for
the broader Soroban contract ecosystem, and deployed-vs-deployed upgrade safety. None
of those are claimed as done here.

## What a grant would accelerate

- Hardening and expanding the static-analysis rule set.
- Broader real-world contract compatibility coverage in CI.
- Developer-onboarding material and a hosted (optional) package index for the
  package-manager workflow.

This document will be updated as scope and maturity change; it deliberately
avoids overstating current capability.
