# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
