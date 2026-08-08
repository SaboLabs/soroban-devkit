# Soroban DevKit (`sdkt`) — Release Readiness

**Version:** `2.5.0` (workspace-wide, single source of truth in `[workspace.package]`)
**Rust edition:** 2021
**MSRV:** `1.88.0` (pinned)
**License:** MIT
**Repository:** https://github.com/naninu123/soroban-devkit
**Default branch:** `main`

This document is the release-readiness snapshot for the current milestone closure.
It is updated whenever a new release tag is cut. It complements `CHANGELOG.md`
(which records *what changed*) with the *current state* of the workspace.

---

## Workspace layout

`sdkt` is a Cargo virtual workspace. The binary `sdkt` is produced by `sdkt-cli`;
all logic lives in focused, dependency-bounded crates.

| Crate | Role |
|-------|------|
| `sdkt-cli` | User-facing CLI (clap + tokio). Routes commands, formats output. Builds the `sdkt` binary. |
| `sdkt-core` | `DevKitConfig`, `NetworkConfig`, `OutputFormat`, validation. No I/O, no networking. |
| `sdkt-xdr` | XDR decode/encode (`ScVal`, `TransactionEnvelope`, `ContractEvent`, …), ABI-aware decoding. No networking. |
| `sdkt-rpc` | `SorobanRpcClient` (persistent pooled `reqwest`), inspect/tx/events/account/sim/submit, Horizon enrichment. |
| `sdkt-storage` | WASM cache, ED25519 identity/keystore, `StorageAnalyzer` (Instance/Persistent/Temporary TTL). |
| `sdkt-wasm` | `ContractSpec` parser, ABI type lookup, WASM metadata, offline diff, `UpgradeVerdict`. |
| `sdkt-audit` | Static security analysis (`AUTH-001/002/003`, `MOVE-001`), `RuleRegistry`, plugin author API. |
| `sdkt-audit-example-rule` | Reference plugin crate (rule `EXAMPLE-001`); loadable as `.so` / `.dylib` / `.wasm`. |

### Dependency graph

```
sdkt-core  ──► (nothing internal)
sdkt-xdr   ──► sdkt-core, sdkt-wasm
sdkt-wasm  ──► sdkt-core
sdkt-rpc   ──► sdkt-core, sdkt-xdr, sdkt-wasm
sdkt-storage ──► sdkt-rpc, sdkt-wasm
sdkt-audit ──► sdkt-wasm
sdkt-audit-example-rule ──► sdkt-audit
sdkt-cli   ──► sdkt-core, sdkt-rpc, sdkt-storage, sdkt-xdr, sdkt-wasm, sdkt-audit, sdkt-audit-example-rule
```

Rule: `sdkt-core` and `sdkt-xdr` perform no networking; everything may depend on them.

---

## Capabilities (shipped)

- **Inspect & decode** — base64 XDR decoding, contract ABI + storage inspection, event exploration.
- **Analyze** — storage TTL / rent visibility, Instance / Persistent / Temporary classification, offline ABI/function/event/type WASM diffing.
- **Secure** — static analysis of contract source (`AUTH-001/002/003`, `MOVE-001`) and an upgrade-safety verdict for safe contract upgrades.
- **Build & ship** — typed transaction envelope builder, simulate, submit, ED25519 keystore, multi-contract workspace topological deployments, and upgrade breaking-change guards.
- **Verify & health** — confirm a deployed contract's on-chain WASM hash matches a local artifact; aggregate posture reports with a `healthy`/`at_risk`/`critical` verdict.
- **Package management** — offline `.sdkt.toml` manifest, git dependency sources, locked dependency resolution (`sdkt.lock`), `package pack` bundles, and `package publish --dry-run` publish-readiness checks.
- **Network profiles** — saved named network configurations (`sdkt network add/list/show/remove`) with M29 precedence (flags > profile > `.sdkt.toml` > defaults).
- **Mainnet safety (M39)** — mutating commands (submit, deploy) refuse mainnet unless the operator explicitly selects the network, guarding against a testnet-default passphrase hitting a mainnet endpoint.
- **Containerized distribution (M39)** — a maintained multi-stage `Dockerfile` (+ `.dockerignore`) producing a minimal, reproducible `sdkt` image.

Most commands are **offline**; only on-chain reads (`inspect`, `storage`, `tx`, `events`, `account`, `fee`, `wasm metadata`, `verify`, `health`) and writes (`tx submit`, `deploy`, `project deploy`) need an RPC endpoint.

---

## Quality gates

All checks below are mandatory for every PR and for every release tag
(`v*`) via `.github/workflows/ci.yml` and `release.yml`. The numbers reflect
The numbers reflect the latest run on `main` at version `2.5.0`.

| Check | Command | Result |
|-------|---------|--------|
| Formatting | `cargo fmt --all --check` | Clean |
| Lint (default) | `cargo clippy --workspace --all-targets -- -D warnings` | Zero warnings |
| Lint (all features) | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Zero warnings |
| Tests | `cargo test --workspace` | 195 passed, 0 failed, 1 ignored |
| MSRV | `cargo check` on pinned `1.88.0` | Passes |
| Install script | `bash -n install.sh` + `bash install.sh --selftest` | Passes |

CI additionally runs on Ubuntu / macOS / Windows matrices, an `install-script`
validation job, and a real-world Soroban compatibility workflow
(`.github/workflows/compatibility.yml`) that builds `stellar/soroban-examples`
contracts and runs `sdkt` against them.

---

## Release process

1. Bump `[workspace.package].version` in the root `Cargo.toml` **and** every
   crate's pinned `version` (internal path-dependencies inherit via
   `*.workspace = true`; the publish-order crates also set their own version).
   `cargo metadata` must report the new version for all `sdkt-*` packages
   (run `cargo metadata --no-deps` to confirm; regenerate `Cargo.lock` if it
   still pins an older version — `Cargo.lock` is tracked intentionally).
2. Update `CHANGELOG.md`: rename `[Unreleased]`'s shipping section to
   `[vX.Y.Z] - YYYY-MM-DD`, and add a fresh `[Unreleased] > ### Planned`
   block.
3. Run the local gates (`fmt`, `clippy -D warnings` default + all-features,
   `test --workspace`). All must be green.
4. Tag exactly matching the workspace version: `git tag vX.Y.Z` and push.
   `release.yml` enforces **tag == `[workspace.package].version`** and
   **built binary version == tag** before any publish.
5. The release workflow builds cross-platform binaries (Linux x86_64,
   macOS x86_64 + aarch64), smoke-tests them offline, generates SHA-256
   checksums, publishes the GitHub Release assets, and sequentially
   `cargo publish`es the 8 crates in dependency order (gated on
   `CARGO_REGISTRY_TOKEN`).

### Publish order (dependency-first)

```
sdkt-core → sdkt-xdr → sdkt-wasm → sdkt-rpc → sdkt-storage → sdkt-audit → sdkt-audit-example-rule → sdkt-cli
```

### Guardrails

- The `sdkt-cli` package must keep the binary name `sdkt` (forbidden to rename
  to `sdkt-cli` for `cargo install` — use `cargo install --path crates/sdkt-cli`).
- Release binaries must report the tag version; mismatches fail the workflow.
- `cargo publish` runs without `--allow-dirty`; a dirty tree fails the release.

---

## Documentation map

| File | Purpose |
|------|---------|
| `README.md` | Project landing page, install, quick start, command table. |
| `ROADMAP.md` | Milestone scope/sequencing (single source of truth). |
| `CHANGELOG.md` | User-facing change history (Keep a Changelog). |
| `docs/quick-start.md` | Five-minute first-run walkthrough. |
| `docs/getting-started.md` + `docs/examples.md` | Deeper examples & CI gating recipes. |
| `docs/installation.md` | Build/install options, feature flags. |
| `docs/cli.md` | Full command reference. |
| `docs/architecture.md` | Crate layout & dependency flow. |
| `docs/compatibility.md` | Real-world contract compatibility matrix. |
| `docs/ci-cd.md` | CI/CD with the reusable composite Action. |
| `docs/plugin-authoring.md` | Extend `sdkt audit` with custom rules. |
| `docs/performance.md` | Offline benchmark baseline. |
| `SECURITY.md` | Supported versions & vulnerability reporting. |
| `CONTRIBUTING.md` | How to contribute. |
| `CODE_OF_CONDUCT.md` | Community standards. |

## Remaining work (deferred)

- **Hosted package registry** — a remote index/server that the `DependencyFetcher`
  trait (M35.1) can target; explicitly deferred past M38 (see "Future Work" in
  `ROADMAP.md`).
- **Remote plugin marketplace layer** — the hosted index/server, remote `https`
  plugin sources, `sdkt plugin update` from a remote, plugin signing / checksum
  verification, and a `.sdktplugin` bundle format. The *local* plugin ecosystem
  (store + install/remove/list/update from local sources, identity-based
  `--rules <id>`) shipped in M40; the remote layer remains unscheduled backlog.
- **Broader Soroban ecosystem integration** — deeper compatibility and
  first-class support for the contracts developers actually deploy.

> Done in M39: containerized distribution (`Dockerfile` + `.dockerignore`),
> mainnet-safety guards on mutating RPC commands, SCF grant positioning
> (`docs/scf.md`), and opt-in `--version` provenance behind the `provenance`
> feature. These items are removed from the deferred list above.

> Done in M40: local offline plugin store (`plugin.toml` metadata), `sdkt plugin
> list/show/install/remove/update` (local-only), and identity-based
> `sdkt audit --rules <id>` resolution. The existing `RuleRegistry`, native
> loader, and WASM (Extism) sandbox are reused unchanged; no hosted registry was
> introduced.
