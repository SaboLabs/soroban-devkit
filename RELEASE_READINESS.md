# Soroban DevKit – Phase 1 Release Readiness Report

## Workspace

- **Workspace layout**: Cargo virtual workspace (`Cargo.toml` in repo root)
- **Crates**:
  - `sdkt-core` (config engine — network + decode settings)
  - `sdkt-xdr` (XDR decoding engine — base64/hex → JSON)
  - `sdkt-cli` (CLI binary — `sdkt decode` subcommand)
- **Rust edition**: 2021
- **MSRV**: Pinned to 1.88.0 in workspace configuration.

## Features Implemented

### sdkt-core
- `DevKitConfig` struct with `NetworkConfig` and `DecodeConfig`
- Default config targeting Soroban testnet
- TOML parsing (`DevKitConfig::from_toml`, `DevKitConfig::from_file`)
- Unit tests for default config and valid TOML parsing

### sdkt-xdr
- `decode()` function: base64/hex string → JSON string
- `decode_bytes()`: raw `&[u8]` → `serde_json::Value`
- Supported types: `ScVal`, `TransactionEnvelope`, `TransactionResult`, `TransactionMeta`, `LedgerKey`, `LedgerEntry`, `ContractEvent`
- Auto-detection across all known types
- `OutputFormat::Json` (compact) and `OutputFormat::Pretty` (default, indented)
- `DecodeError` enum with variants for Base64, Hex, XDR parse, Unknown type, Invalid input, and JSON serialization failures
- Unit tests covering empty payload, invalid base64 input, valid ScVal decoding, auto-detection, type hints, and JSON/Pretty format difference
- Doc test verifying the `decode()` function example

### sdkt-cli
- `sdkt decode` subcommand via Clap v4 derive macros
- Inline argument input
- File input (`-i/--file <FILE>`)
- Type hint override (`--type`)
- Output format selection (`--format json|pretty`, default `pretty`)
- Pretty error propagation via `Box<dyn std::error::Error>`

## Issues Fixed During Phase 1

| # | Issue | File | Resolution |
|---|-------|------|------------|
| 1 | `assert` used as statement instead of macro | `crates/sdkt-xdr/src/lib.rs` line 228 | Changed `assert(...)` to `assert!(!compact.contains('\n'))` |
| 2 | Redundant license files committed | `LICENSE-MIT`, `LICENSE-APACHE` | Deleted via `git rm --cached` |
| 3 | Missing baseline documentation | Root files | Created README.md, CHANGELOG.md, CONTRIBUTING.md, SECURITY.md, CODE_OF_CONDUCT.md, LICENSE, .gitignore |

## Quality Checks

| Check | Command | Result |
|-------|---------|--------|
| Formatting | `cargo fmt --check` | Clean (no changes needed) |
| Linting | `cargo clippy --workspace -- -D warnings` | Zero warnings, zero errors |
| Build (test profile) | `cargo build` | Finished successfully |
| Tests | `cargo test --workspace` | 9 tests passed, 0 failures |
| Doc tests | Included in test run | 1 doc test passed |

Test Summary:

| Crate | Unit tests | Doc tests | Failures |
|-------|------------|-----------|----------|
| sdkt-cli | 0 | 0 | 0 |
| sdkt-core | 2 | 0 | 0 |
| sdkt-xdr | 6 | 1 | 0 |
| **Total** | **8** | **1** | **0** |

Clippy Summary:

- Warnings: 0
- Errors: 0
- Status: Clean

## Documentation

| File | Present |
|------|---------|
| README.md | Yes |
| CHANGELOG.md | Yes |
| CONTRIBUTING.md | Yes |
| SECURITY.md | Yes |
| CODE_OF_CONDUCT.md | Yes |
| LICENSE | Yes (MIT) |
| .gitignore | Yes |
| GAP_ANALYSIS.md | Pre-existing (retained) |

## Git Status

- **Current branch**: `main`
- **Staged (new)**: `.gitignore`, `CHANGELOG.md`, `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`, `LICENSE`, `README.md`, `SECURITY.md`
- **Staged (deleted)**: `LICENSE-APACHE`, `LICENSE-MIT` (removed as duplicates)
- **Modified (source + build artifacts)**: `Cargo.lock`, `crates/sdkt-cli/Cargo.toml`, `crates/sdkt-cli/src/main.rs`, `crates/sdkt-xdr/src/lib.rs`, `target/debug/sdkt-cli` plus many `target/debug/incremental/` files (will be ignored in future once `.gitignore` includes `/target`)
- **Untracked**: Numerous `target/debug/.fingerprint/` and `target/debug/deps/` entries (build output — covered by `.gitignore`)

Note: Git status is NOT clean — build artifacts and lockfile are modified. All intentional documentation and license files are staged and ready.

## Release Checklist

The repository adheres to semantic versioning (SemVer). Before tagging a new release, verify the following:

- [ ] **Dependencies updated:** Cargo workspace lockfile (`Cargo.lock`) is clean and `cargo update` has been run if needed.
- [ ] **MSRV verified:** `cargo check` passes on the pinned MSRV (`1.88.0`).
- [ ] **Lint and Tests:** `cargo fmt --check`, `clippy -D warnings`, and `cargo test --workspace` all pass cleanly locally.
- [ ] **Changelog updated:** The `CHANGELOG.md` file has the `[Unreleased]` block renamed to `[vX.Y.Z] - YYYY-MM-DD`.
- [ ] **Versions bumped:** Workspace members in all `Cargo.toml` files are bumped to the new version `X.Y.Z` (internal path dependencies updated).
- [ ] **Release Readiness Notes:** The `RELEASE_READINESS.md` file reflects the active milestone closures.
- [ ] **Smoke Test Passing:** The `.github/workflows/release.yml` native execution smoke tests succeeded in the previous dry-run or will succeed upon tagging.

**Execution:**
When the checklist is complete, tag the commit (`git tag vX.Y.Z`) and push. The `release.yml` GitHub action will compile, smoke-test, checksum, and deploy the binaries to the GitHub Release page, followed by sequential crates.io publishing.

## Remaining Work

The following items are deferred to future milestones and were **not** completed in Phase 1:

- Docker image for containerized runs — planned for Phase 4
## GitHub Actions CI Pipeline

- `ci.yml` — Runs Rust `fmt`, `clippy`, and `test` workflows across Ubuntu, macOS, and Windows matrices.
- `release.yml` — Orchestrates cargo publishing and GitHub release asset packaging.

## Next Milestone (Phase 2 — Storage & Inspection)

Phase 2 targets two core lifecycle tools that are missing from the Stellar/Soroban ecosystem:

1. **Storage rent visibility** — `sdkt storage check <contract-id>` returns TTL timeline and extension cost, preventing silent contract expiration.
2. **Contract inspection** — `sdkt inspect <contract-id>` reads WASM custom sections and current on-chain storage state.

Both will be implemented as new subcommands in `sdkt-cli`, backed by RPC calls against the configured network. Unit tests and integration points will be added to each crate as appropriate.