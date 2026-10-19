# Benchmark Plan
This document outlines the criteria for performance testing Soroban DevKit. No premature optimization will occur until these baseline metrics are captured via std benchmarking or isolated test setups.

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

## Offline Harness (`scripts/bench_offline.sh`)

The offline harness measures three commands against a release `sdkt` binary:

1. `sdkt wasm inspect`
2. `sdkt audit`
3. `sdkt diff --upgrade-safety`

Each command is run multiple times and the harness reports wall-clock latency stats (min/median/average) and peak resident set size (RSS). The output format is stable so existing tooling that parses it continues to work: one `printf` line per command, preceded by the `pin=` and `rss=` probe lines.

### Fixtures

By default the harness uses committed fixtures under `scripts/fixtures/` so it runs out of the box on any platform:

- `scripts/fixtures/wasm/` — `token.wasm`, `liquidity_pool.wasm`, `atomic_swap.wasm`, `timelock.wasm` for `wasm inspect` and `diff --upgrade-safety`.
- `examples/sample_token/src/lib.rs` — self-contained Rust source for `audit` (deterministic `AUTH-001` finding).

You can override these with the `WASM_DIR` and `AUDIT_SRC` environment variables. To rebuild the WASM fixtures from the pinned `stellar/soroban-examples` commit, run `scripts/fetch_bench_fixtures.sh` (fetches sources and builds them with `stellar contract build`; upstream publishes no pre-built WASM).

### RSS portability

Peak RSS is collected differently depending on the platform. The harness never silently emits an empty RSS value as if it were zero:

- **GNU time** (`time -v`, found on `PATH`): used when present; RSS is read from the `Maximum resident set size` field.
- **Linux without GNU time**: falls back to polling `/proc/<pid>/status` for `VmHWM`.
- **macOS without GNU time**: falls back to `getrusage(RUSAGE_CHILDREN)` (`ru_maxrss`, bytes converted to kB) via a small Python helper.
- **Windows or other platforms without a portable RSS probe**: RSS is reported as `unavailable`, and wall-clock stats are still produced. `RSS_MODE` can be set explicitly to exercise a specific probe.

Wall-clock is measured on the bare command; RSS sampling runs alongside it, so probe overhead never inflates the timings.

## Methodology

- Build the release binary: `cargo build --release --bin sdkt`.
- Run `bash scripts/bench_offline.sh` on a quiet machine.
- Record the `cpu` model, OS, and release tag alongside the numbers in `docs/advanced/performance.md`.
- Re-run the harness for each release and update the baseline table; flag regressions against the previous baseline.
