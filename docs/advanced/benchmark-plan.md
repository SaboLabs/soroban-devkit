# Benchmark Plan

This document outlines the criteria for performance testing Soroban DevKit. No premature optimization will occur until these baseline metrics are captured via the offline benchmark harness (`scripts/bench_offline.sh`) or isolated test setups.

## Methodology

The offline harness runs the release `sdkt` binary against committed fixtures and measures three commands: `wasm inspect`, `audit`, and `diff --upgrade-safety`. For each command it records wall-clock time and peak resident set size (RSS).

- **Wall-clock time** is always measured.
- **Peak RSS** is measured via GNU time (`/usr/bin/time -v`) when available, falling back to a portable probe on Linux (`/proc` polling) and macOS (`rusage`). Where no portable probe exists (e.g. Windows), RSS is reported as `unavailable` rather than as zero. See `docs/advanced/performance.md` for the per-platform details.
- Output format is stable so existing tooling that parses the harness output continues to work.

Baselines are recorded per release and per machine profile; comparisons are only meaningful against a baseline captured on a comparable machine.

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
