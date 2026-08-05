# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v0.11.0-alpha] - 2026-08-05 (Milestone 11 — IMPLEMENTED, MERGED to main)

### Added (Proposal B: finish `StorageAnalyzer`)
- **`sdkt storage analyze <contract-id>`** — categorizes a contract's storage into Instance / Persistent / Temporary entries, with a TTL summary and per-entry detail. Pretty + JSON via existing `OutputFormat`.
- `sdkt-storage`: real Instance/Persistent/Temporary classification by decoding the XDR `LedgerKey` (`StorageClass`); `StorageEntry` per-entry detail added to `StorageReport`.
- 5 unit tests (classification round-trips for instance/persistent/temporary/invalid) + 3 `sdkt-cli` integration tests for `storage analyze`.
- Additive, backwards-compatible: `StorageReport` gains `total_entries`, `other_entries`, `entries` (serde-defaulted) — no breaking changes.

> Note: M11 was scoped to Proposal B only (per operator approval). `sdkt-audit` (Gap C)
> and the plugin framework (M13) were explicitly excluded. See `docs/milestone-11-plan.md`
> for the (unapproved) audit candidate design.

## [v0.10.0-alpha] - 2026-08-05 (Milestone 10 / ENG-16 — MERGED to main)

### Added
- **ABI-aware decoding**: `--abi <WASM>` flag on `events`, `inspect`, and `storage check`.
- `sdkt-wasm` `ContractSpec` parser; `sdkt-xdr::decode_event_topics` for event topic/value decoding.
- Real base64 XDR event topic + data-value decoding (previously a no-op stub).
- `sdkt_xdr::scval_from_base64` helper for decoding event payloads.
- ABI functions/events/custom-types display in pretty + JSON output.

### Fixed
- `events --abi` now decodes actual topics/value instead of empty vectors.
- Removed accidentally-committed 4.6 MB `gen_keys` binary; added to `.gitignore`.
- Clippy-clean (`-D warnings`) across workspace.

## [v0.9.0-alpha] - 2026-08-05 (Milestone 9)

### Added
- **WASM tooling**: `sdkt wasm metadata` and `sdkt wasm cache` (info/remove/clear).
- `sdkt-wasm` crate: `ContractSpec` parser for `contractspecv0` / `contractenvmetav0` sections.
- `sdkt deploy` (upload WASM + instantiate) and `sdkt init` project scaffolding engine.
- WASM metadata caching in `sdkt-storage` (`WasmCache`).
- Identity/keystore foundation reused by deploy/init flows.

## [v0.8.0-alpha] - 2026-08-04 (Milestone 8 — Mutability Foundation)

### Added
- **Transaction simulation**: `sdkt tx simulate` (offline pre-flight via `simulateTransaction`).
- **Transaction submission**: `sdkt tx submit` with optional wait/poll (`submit_and_wait`).
- **Identity / keystore**: `sdkt identity` generate/import/list/show/delete/default (ED25519, `~/.sdkt/identities`).
- **Envelope builder**: `sdkt tx build` (typed arg parsing → base64 XDR envelope).
- **Fee estimation**: `sdkt fee estimate` (RPC dynamic fee or manual base-fee samples).
- Validation module (`sdkt_core::validation`) for offline envelope checks.

## [v0.7.0-alpha] - 2026-08-04 (Milestone 7)

### Added
- **Horizon account enrichment**: `sdkt account` now pulls balances, signers, and associated assets via Stellar Horizon REST.
- **ScVal pretty-print UI**: human-readable ScVal rendering in CLI pretty output (improved readability for events/storage values).

## [v0.6.0-alpha] - 2026-08-04

### Improved
- **Production Readiness**: Hardened `sdkt-rpc` client timeout handling and mapping boundaries.
- **RPC Resilience**: Added internal retry mechanisms to gracefully handle short-lived network interruptions.
- **Documentation Quality**: Maximum structural rustdoc coverage across core workspace crates and updated README with benchmark planning.

### Testing
- Validation completed on workspace boundaries with Clippy strict policies.
- CLI integration tests increased to cover transaction, storage, and account edge-cases.

### Internal
- Workspace DRY cleanups (unified generic `.request()` methods).
- GitHub Actions workflow added to execute formatting and clippy automated checks.

## [v0.5.0-alpha] - 2026-08-04

### Added
- **Transaction Inspection**: `sdkt tx inspect` command to view transaction hash, status, ledger inclusion, and operation counts.
- **Event Explorer**: `sdkt events` command to fetch and list emitted Soroban contract events.
- **Account Inspection**: `sdkt account` command for base level diagnostics of Stellar/Soroban accounts.
- **Generic RPC request abstraction**: `SorobanRpcClient` now exposes a public `request()` method for generic JSON-RPC interactions.
- **Integration Tests**: Comprehensive test suite coverage added for `tx`, `events`, and `account` commands using `assert_cmd`.

### Improved
- **CLI Architecture**: Hardened separation between CLI output formatting and RPC business logic.
- **RPC Abstraction**: Centralized API request formatting and internal error handling mapping inside the RPC crate.
- **Documentation**: Substantial overhaul of README.md and internal milestone documentation outlining workspace boundaries.

### Internal
- Workspace cleanup, dependency deduplication, and module flattening.
- Implemented robust struct-based API boundaries in `sdkt-rpc`.
- Enforced 100% strict test coverage mapping for CLI boundaries.

## [v0.4.0-alpha] - 2026-08-04

### Added
- Initial Soroban RPC inspection tools (`sdkt inspect`).
- Base64 XDR parser via `sdkt-xdr` (`sdkt decode`).
- Storage TTL analysis (`sdkt storage check`).
- Basic workspace architecture and integration testing.
