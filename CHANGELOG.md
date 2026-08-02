# Changelog

All notable changes to the Soroban DevKit project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-08-01

Initial baseline release (MVP).

### Added
- XDR decode command (`sdkt decode <base64-or-hex>`)
- Support for ScVal, TransactionEnvelope, TransactionResult, TransactionMeta, LedgerKey, LedgerEntry, and ContractEvent
- Auto-detection of common XDR types
- TOML configuration system (`.sdkt.toml`)
- JSON and pretty-print output formats
- File-based input (`--file`) for piping and scripting
- Unit tests for core and XDR crates (9 tests total)
