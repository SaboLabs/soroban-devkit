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

- 37 milestones merged to `main` (storage/inspect foundation through M44 — the
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

## Honest current traction (verified)

All figures below were verified directly from the package registry and the
repository's GitHub API on 2026-08-15. No external adoption, production usage,
testimonials, or partnerships are claimed.

- **Crates published:** all 8 workspace crates are published to crates.io at
  `v2.5.0` (verified via `crates.io/api/v1/crates/<name>`).
- **Crates.io downloads (cumulative, verified):** sdkt-core 125, sdkt-xdr 107,
  sdkt-wasm 107, sdkt-rpc 101, sdkt-storage 87, sdkt-cli 68, sdkt-audit 73,
  sdkt-audit-example-rule 52. These are early-stage install counts, not active
  user counts.
- **GitHub repository (verified via API):** 2 stars, 1 fork, 1 open issue.
- **Releases:** 14 GitHub releases (per prior M49 verification); release
  downloads are minimal/zero on GitHub assets (binaries distributed via
  crates.io + install.sh).
- **External dependency usage / ecosystem adoption:** NOT VERIFIED. The following
  checks were performed on 2026-08-15 and found no external usage:
  - **crates.io reverse dependencies:** every reverse dependency for all 8 crates
    resolves to an internal workspace sibling (sdkt-cli, sdkt-core, sdkt-xdr,
    sdkt-wasm, sdkt-rpc, sdkt-storage, sdkt-audit, sdkt-audit-example-rule). No
    external crate depends on any of them.
  - **GitHub code search** for `SaboLabs/soroban-devkit` in `Cargo.toml` requires
    authentication (401 unauthenticated) and could not be run; the unauthenticated
    **repository search** for the quoted string `"SaboLabs/soroban-devkit"` returns
    exactly 1 result — the project itself. No external public repository references it.
  - **Forks:** 1 (Shadow-MMN/soroban-devkit) — a fork of this repo, not external
    adoption.
  - **Name-collision note:** generic searches for "soroban-devkit" surface unrelated
    projects (e.g. `sorocore/soroban-devkit`, a 0-star TypeScript toolkit). These
    are FALSE POSITIVES, not adoption of this Rust/CLI project.
  - This is marked **UNKNOWN → verified absent** for external *dependency* usage
    (no evidence found via available unauthenticated methods), not assumed present.

This is an early-stage, single-maintainer developer tool. The evidence above is
technical and distribution-level, not usage-based.

## Maintainer / credibility (verified, no claims of employment or affiliation)

- The project is developed by a single maintainer (GitHub `naninu123`, public as
  `sabo`; git history shows one human author). The associated `SaboLabs` GitHub
  organization describes itself as "Independent security research & automation.
  Web3 audits, bug bounties, autonomous agents."
- The maintainer has multiple **public** Stellar/Soroban repositories
  (e.g. `Soroban-Contract-Explorer`, `soroban-security-portal`,
  `Grainlify-Stellar-Contracts`, `Stellar-forge`), indicating prior ecosystem
  involvement. These are cited as capability/context evidence only — not as
  adoption, partnership, or endorsement of `sdkt`.
- No corporate affiliation, funding, or Stellar Foundation endorsement is claimed.

This document will be updated as scope and maturity change; it deliberately
avoids overstating current capability.
