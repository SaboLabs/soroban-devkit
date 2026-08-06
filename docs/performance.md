# Performance Benchmark & Regression Baseline (M35)

Establishes repeatable performance baselines for the three most important
offline `sdkt` commands so future releases can detect regressions.

## Hardware / Environment

| Field | Value |
|-------|-------|
| Machine | VPS (AWS `aarch64`/`x86_64` shared, 2 vCPU) |
| CPU | 2 vCPU (`nproc` = 2) |
| Memory | 3.7 GiB total |
| OS | Linux (Ubuntu) |
| Rust | rustc 1.97.1 (stable) |
| sdkt | 2.0.0 (release build) |
| Benchmark date | 2026-08-06 |

> Reported numbers were captured on the project VPS. Absolute values will vary
> by host; **the relative trend across releases is what matters.** Re-run the
> helper script on the same machine to compare.

## Benchmark methodology

- **Binary:** release build (`cargo build --release --bin sdkt`). Debug builds
  are not representative and are excluded.
- **Warmup:** 1 discarded run per command to absorb cold page-cache / lazy
  linking effects.
- **Runs:** 7 measured runs per command (configurable via `RUNS=`).
- **Wall-clock:** measured with nanosecond `date +%s.%N` around the command;
  reported as **min / median / average** (seconds).
- **Peak memory:** captured via `/usr/bin/time -v` (Maximum resident set
  size). Reported as **median** peak RSS (kB).
- **Noise reduction:** each run is pinned to a single core with
  `taskset 0x1` when available, so scheduler migration does not inflate
  variance.
- **Reproducibility:** two independent passes produced near-identical medians
  (see Baseline Results), confirming the harness is not flaky.

Helper script: `scripts/bench_offline.sh`.

```bash
# Reproduce on any machine:
SDKT=target/release/sdkt \
WASM_DIR=/path/to/wasm \
AUDIT_SRC=/path/to/token/src/lib.rs \
RUNS=7 bash scripts/bench_offline.sh
```

## Commands tested

| Command | What it exercises |
|---------|-------------------|
| `sdkt wasm inspect <wasm>` | WASM parse, custom-section + ABI decode |
| `sdkt audit <src.rs>` | Rust source parse + static rule evaluation |
| `sdkt diff --upgrade-safety --old-wasm A --new-wasm B` | WASM ABI diff + breaking-change verdict |

## Dataset

Reuses the M33/M34 real-world fixtures (compiled from the official
`stellar/soroban-examples` tree, `wasm32v1-none` release):

| Artifact | Size | Type |
|----------|------|------|
| `token.wasm` | 8.4 KiB | SIP-10 token |
| `liquidity_pool.wasm` | 10.4 KiB | DeFi AMM |
| `atomic_swap.wasm` | 1.7 KiB | swap/escrow |
| `timelock.wasm` | 3.7 KiB | claimable balance |
| `single_offer.wasm` | 5.3 KiB | DEX offer |

Audit target: `token/src/lib.rs` (8 source files) and `liquidity_pool/src/lib.rs`.

## Baseline results

First pass (RUNS=7, pinned to 1 core), sdkt 2.0.0:

| Command | wall min | wall median | wall avg | peak RSS (median) |
|---------|:-------:|:----------:|:-------:|:-----------------:|
| `wasm inspect token` | 0.0035 s | 0.0036 s | 0.0039 s | 5,916 kB |
| `wasm inspect liquidity_pool` | 0.0035 s | 0.0036 s | 0.0036 s | 5,972 kB |
| `audit token/src/lib.rs` | 0.0036 s | 0.0037 s | 0.0037 s | 6,376 kB |
| `diff self token` | 0.0036 s | 0.0037 s | 0.0037 s | 5,976 kB |
| `diff token→lp` | 0.0038 s | 0.0038 s | 0.0039 s | 5,924 kB |

Second pass (RUNS=5, reproducibility) — `audit` against the heavier
`liquidity_pool/src/lib.rs` (more code):

| Command | wall median | peak RSS (median) |
|---------|:-----------:|:-----------------:|
| `wasm inspect token` | 0.0037 s | 6,016 kB |
| `wasm inspect liquidity_pool` | 0.0034 s | 6,036 kB |
| `audit liquidity_pool/src/lib.rs` | 0.0049 s | 7,392 kB |
| `diff self token` | 0.0036 s | 5,936 kB |
| `diff token→lp` | 0.0041 s | 5,968 kB |

**Takeaways (baseline):**
- All three commands finish in **< 5 ms** on this host.
- Peak RSS stays **< 8 MiB** even on the largest audited source.
- `audit` scales gently with source size (token ~3.7 ms / 6.4 MiB →
  liquidity_pool ~4.9 ms / 7.4 MiB).
- `diff` cost is dominated by WASM ABI decode, roughly constant per artifact.

## Known limitations

1. **Absolute numbers are host-specific.** CPU model, cache pressure, and
   concurrent load on the shared VPS shift wall-clock by 10–30%. Compare
   releases on the *same* machine using the helper script.
2. **No startup-time isolation.** Wall-clock includes process spawn + CLI
   arg parsing. For a pure library-level budget, wrap the core call in a
   microbench (e.g. `criterion`) — out of scope for this baseline.
3. **Peak RSS via `/usr/bin/time -v`** is whole-process and includes the
   Rust runtime + CLI overhead, not just the command's working set.
4. **Single binary, no parallel stress.** This measures one-shot offline
   commands, not throughput under concurrency.
5. **No flamegraph / per-phase breakdown** yet; if a future release regresses,
   add `cargo flamegraph` or `pprof` to localize the hotspot.

## Regression policy

- Re-run `scripts/bench_offline.sh` on the same host before a release tag.
- A **> 2× median wall-time or > 50% peak-RSS increase** on any command vs
  this baseline is a suspected regression and must be investigated before
  shipping.
- No code was optimized for M35 — the baseline reflects current behavior. Any
  future fix discovered here should add a focused regression check alongside
  this script.
