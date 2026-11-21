# Benchmark Plan

This document outlines the criteria for performance testing Soroban DevKit. No premature optimization will occur until these baseline metrics are captured via the offline benchmark harness (`scripts/bench_offline.sh`) or isolated test setups.

## Methodology

The offline harness runs the release `sdkt` binary against committed fixtures and measures three commands: `wasm inspect`, `audit`, and `diff --upgrade-safety`. For each command it records wall-clock time and peak resident set size (RSS).

- **Wall-clock time** is always measured. On Linux and macOS the harness uses a portable clock (`perl -MTime::HiRes` when available, otherwise whole-second `date +%s`), since macOS `date` does not support `%N`.
- **Peak RSS** is measured via GNU time (`/usr/bin/time -v`) on Linux and BSD time (`/usr/bin/time -l`) on macOS when available. When neither is present, the harness reports RSS as `unavailable` rather than as zero. Windows is not supported by the offline harness and reports RSS as `unavailable`. See `docs/advanced/performance.md` for the per-platform details.
- Output format is stable so existing tooling that parses the harness output continues to work.

Baselines are recorded per release and per machine profile; comparisons are only meaningful against a baseline captured on a comparable machine.

## Fixtures

The harness reads its default fixtures from `bench/fixtures/` at the repository root. If these fixtures are absent on a fresh checkout, the harness exits with an explicit error; fixtures can be supplied via the environment overrides documented in `scripts/bench_offline.sh`.

## Benchmark Areas

### 1. XDR Decoding (`sdkt-xdr`)
**Focus**: Throughput of `ScVal` decodes and parsing of sponsored large `LedgerEntry` blocks.
**Metric**: Baseline decode latency for varying payload lengths.

### 2. RPC Latency (`sdkt-rpc`)
**Focus**: Overhead injected by our reqwest endpoint construction, retries, and HTTP timeouts.
**Metric**: Roundtrip time for `getHealth`, `getLedger` across local testnets.

### 3. Storage Analysis (`sdkt-storage` / `sdkt-rpc`)
**Focus**: CPU time mapping XDR count limits against Wasm mapping representations.

### 4. Transaction Inspection (`sdkt-rpc`)
**Focus**: Latency added when fetching bulk transaction history vs caching result mappings.
