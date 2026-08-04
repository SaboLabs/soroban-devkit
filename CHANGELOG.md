# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0-alpha] - 2026-08-04

### Added
- Soroban RPC inspection: Added `sdkt inspect` command to check contract WASM hash and fetch storage keys via Soroban RPC.
- Storage analysis: Added `sdkt storage check` to calculate and analyze storage entry lifetimes.
- TTL visibility: Added TTL output including remaining ledgers and expiration time estimations.
- JSON output: Supported `--format json` output option for integration with other tools.
- Unified SDK core, XDR decoding, and storage inspection crates under a single workspace.
