# Benchmark Plan

This document outlines the criteria for performance testing Soroban DevKit. No premature optimization will occur until these baseline metrics are captured via std benchmarking or isolated test setups.

The offline benchmark harness is `scripts/bench_offline.sh`. It measures three commands: `wasm inspect`, `audit`, and `diff --upgrade-safety`, and reports wall-clock statistics plus peak RSS per run.

## Methodology

- **Wall-clock time**: measured for every run and always reported.
- **Peak RSS**: measured with GNU `/usr/bin/time -v` when available. Where GNU time is absent, the harness uses a portable probe (`/proc` polling on Linux, `rusage` via `getrusage` on macOS). If neither is available, RSS is reported as `unavailable`; it is never emitted as empty or zero.
- **Fixtures**: `WASM_DIR` and `AUDIT_SRC` default to committed fixtures in the repository. A documented fetch step is available for the `soroban-examples` WASM files.
- **Baselines**: results are recorded per release in `docs/advanced/performance.md`, together with the benchmark date and machine profile.

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
