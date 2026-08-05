# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v0.15.0-alpha] - 2026-08-05 (Milestone 15 — CI/CD GitHub Action)

### Added
- **Reusable GitHub composite Action** — `.github/actions/sdkt/action.yml` wraps existing `sdkt` capabilities for CI: `command: audit` runs `sdkt audit <target> --format json` and fails when findings meet `severity-threshold` (default `critical`, so `MOVE-001` warnings never break CI); `command: upgrade-safety` runs `sdkt diff --old-wasm <old> --new-wasm <new> --upgrade-safety --format json` and fails when `compatible == false`.
- **Action self-validation workflow** — `.github/workflows/sdkt-action-ci.yml` exercises the composite Action against the committed WASM fixtures: a breaking diff (`us_old.wasm` → `us_new.wasm`) is asserted to fail, and an identical diff is asserted to pass.
- **Documentation** — `docs/ci-cd.md` with copy-paste workflow examples (audit-on-PR, upgrade-safety-on-release, self-validation) plus install/threshold notes.
- Packaging only: no new crate, no Rust changes, no breaking API changes. Reuses the M13 `sdkt audit` and M14 `sdkt diff --upgrade-safety` JSON contracts.

## [v0.14.0-alpha] - 2026-08-05 (Milestone 14 — Upgrade Safety Guard)

### Added
- **`sdkt diff --upgrade-safety`** — transforms the M12 `SpecDiff` into an actionable `UpgradeVerdict`: `breaking_changes` (removed function, changed signature, removed event, removed type) vs `non_breaking_changes` (additions). Pretty + JSON via existing `--format`.
- `sdkt-wasm`: `UpgradeVerdict`, `VerdictChange`, `ChangeKind`, `upgrade_safety()` / `upgrade_safety_wasm()` — all derived from the existing `diff_specs`/`SpecDiff` (no duplicated comparison logic).
- **`sdkt deploy --deny-breaking --old-wasm <deployed.wasm>`** — optional deploy guard that aborts when the upgrade is not backwards-compatible. Off by default; existing `deploy` behavior unchanged when the flag is omitted.
- 6 unit tests (`upgrade_safety`: removed fn, changed signature, removed event, removed type, additions-only, identical) + 5 `sdkt-cli` integration tests (pretty, JSON, `deploy --deny-breaking`).
- Additive, backwards-compatible: new types + new flags only; no breaking API changes.

## [v0.13.0-alpha] - 2026-08-05 (Milestone 13 — Gap C: Static Security Analysis)

### Added
- **`sdkt audit <path>`** — offline static security analysis of a Soroban contract Rust source. Flags `AUTH-001` (missing `require_auth` on privileged fns), `AUTH-002` (unauthenticated `invoke_contract`), `AUTH-003` (unguarded `initialize`), and `MOVE-001` (suspicious move-after-use, Warning only). Pretty + JSON via existing `--format`.
- New crate **`sdkt-audit`**: `Severity`, `Finding`, `AuditReport`, `AuditRule` trait, `audit_source()` / `audit_source_with()` / `audit_source_with_spec()` (reuses `sdkt-wasm::ContractSpec` for cross-checking). Built-in rules are additive and `--disable`-able.
- 13 unit tests (per-rule positives/negatives, disable, clean, parse-error) + 6 `sdkt-cli` integration tests for `audit`.
- Additive, backwards-compatible: new crate + new CLI subcommand; no breaking API changes; `sdkt-core` remains networking-free.

## [v0.12.0-alpha] - 2026-08-05 (Milestone 12 — Contract ABI/WASM Diff, Candidate C)

### Added
- **`sdkt diff --old-wasm <A> --new-wasm <B>`** — offline comparison of two contract WASM binaries. Reports added/removed functions, changed function signatures, added/removed events, and added/removed custom types. Pretty + JSON via existing `OutputFormat`.
- `sdkt-wasm`: new `spec_diff` module with `diff_wasm()` / `diff_specs()` and a serializable `SpecDiff` report (per-WASM SHA-256 hash + size context). Reuses the existing `parse_contract_spec` parser.
- 7 unit tests (added/removed/changed functions, events, types, identical-spec, parse-error propagation) + 3 `sdkt-cli` integration tests for `diff`.
- Additive, backwards-compatible: new module + re-exports; no breaking API changes; no new crates.

## [v0.11.0-alpha] - 2026-08-05 (Milestone 11 — StorageAnalyzer, Proposal B)

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
