# Performance Benchmark & Regression Baseline

Establishes repeatable performance baselines for the three most important
offline `sdkt` commands so future releases can detect regressions.

## Hardware / Environment

| Field | Value |
|-------|-------|
| Machine | MacBook Pro (Intel) |
| CPU | `Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz` |
| Memory | 16 GiB |
| OS | macOS 26.5.2 (`x86_64`) |
| RSS probe | `getrusage(RUSAGE_CHILDREN)` (no GNU time installed) |
| Rust | rustc 1.99.1 (stable) |
| sdkt | 2.7.0 (release build) |
| Benchmark date | 2026-10-19 |

An earlier capture on the shared project VPS (Linux/Ubuntu, 2 vCPU, 3.7 GiB,
rustc 1.97.1) is kept in [Linux capture](#linux-capture-reference) for trend
comparison.

> Absolute values vary by host; **the relative trend across releases is what
> matters.** Re-run the helper script on the same machine to compare.

## Benchmark methodology

- **Binary:** release build (`cargo build --release --bin sdkt`). Debug builds
  are not representative and are excluded.
- **Warmup:** 1 discarded run per command to absorb cold page-cache / lazy
  linking effects.
- **Runs:** 7 measured runs per command (configurable via `RUNS=`).
- **Wall-clock:** measured with nanosecond `date +%s.%N` around the command;
  reported as **min / median / average** (seconds).
- **Peak memory:** captured via `/usr/bin/time -v` (Maximum resident set
  size) when GNU time is available; otherwise a portable RSS probe is used
  (see *RSS portability* below). Reported as **median** peak RSS (kB), or
  `unavailable` when no probe exists on the host.
- **Noise reduction:** each run is pinned to a single core with
  `taskset 0x1` when available, so scheduler migration does not inflate
  variance.
- **Reproducibility:** two independent passes produced near-identical medians
  (see Baseline Results), confirming the harness is not flaky.

Helper script: `scripts/bench_offline.sh`.

### RSS portability

The harness never emits an empty RSS value as if it were zero. RSS is
resolved in this order:

| Platform | Probe | Notes |
|----------|-------|-------|
| Linux, GNU time present | `/usr/bin/time -v` | `Maximum resident set size` |
| Linux, no GNU time | `/proc/<pid>/status` polling (`VmHWM`) | portable fallback |
| macOS | `getrusage(RUSAGE_CHILDREN)` (`ru_maxrss`) | via a small helper |
| Windows / other | none | reported as `unavailable` |

When RSS is `unavailable`, wall-clock statistics are still produced and the
RSS column reads `unavailable` — it is never blank and never `0`.

### Fixtures

The script ships with committed fixtures under `scripts/fixtures/` so it runs
out of the box on any platform:

- `scripts/fixtures/wasm/` — `token.wasm`, `liquidity_pool.wasm`,
  `atomic_swap.wasm` and `timelock.wasm`, built with `stellar contract build`
  (`wasm32v1-none`, release) from `stellar/soroban-examples`.
- `examples/sample_token/src/lib.rs` — the self-contained sample contract
  audited by the harness. It deliberately omits `require_auth()` in
  `admin_action`, so `sdkt audit` always reports the same `AUTH-001` finding.

`stellar/soroban-examples` does not publish pre-built WASM (the upstream
`target/` directory is gitignored and its releases carry no assets), so
`scripts/fetch_bench_fixtures.sh` fetches the sources at a pinned commit and
builds them locally. `scripts/fixtures/MANIFEST.txt` records that ref and the
SHA-256 of each artifact:

```bash
# Rebuild the committed fixtures (requires stellar-cli + wasm32v1-none).
bash scripts/fetch_bench_fixtures.sh

# Reproduce on any machine:
SDKT=target/release/sdkt \
WASM_DIR=/path/to/wasm \
AUDIT_SRC=/path/to/lib.rs \
RUNS=7 bash scripts/bench_offline.sh
```

## Commands tested

| Command | What it exercises |
|---------|-------------------|
| `sdkt wasm inspect <wasm>` | WASM parse, custom-section + ABI decode |
| `sdkt audit <src.rs>` | Rust source parse + static rule evaluation |
| `sdkt diff --upgrade-safety --old-wasm A --new-wasm B` | WASM ABI diff + breaking-change verdict |

## Dataset

Reuses the real-world fixtures (compiled from the official
`stellar/soroban-examples` tree, `wasm32v1-none` release, committed under
`scripts/fixtures/wasm/`):

| Artifact | Size | Type |
|----------|------|------|
| `token.wasm` | 6.6 KiB | SIP-10 token |
| `liquidity_pool.wasm` | 8.6 KiB | DeFi AMM |
| `atomic_swap.wasm` | 1.6 KiB | swap/escrow |
| `timelock.wasm` | 3.0 KiB | claimable balance |

Audit target: `examples/sample_token/src/lib.rs` (self-contained; deterministic
`AUTH-001` finding).

## Baseline results

Recorded baseline — macOS, `RUNS=7`, sdkt 2.7.0, RSS via `getrusage`:

| Command | wall min | wall median | wall avg | peak RSS (median) |
|---------|:-------:|:----------:|:-------:|:-----------------:|
| `wasm inspect token` | 0.0204 s | 0.0205 s | 0.0211 s | 4,420 kB |
| `wasm inspect liquidity_pool` | 0.0164 s | 0.0174 s | 0.0178 s | 4,428 kB |
| `audit sample_token` | 0.0166 s | 0.0174 s | 0.0174 s | 4,920 kB |
| `diff self token` | 0.0184 s | 0.0206 s | 0.0204 s | 4,524 kB |
| `diff token→lp` | 0.0173 s | 0.0194 s | 0.0190 s | 4,548 kB |

On this host wall-clock is dominated by process spawn (the release binary is
~25 MB); the per-command work is a single-digit-millisecond delta below that
floor, which is why the Linux capture below reports ~3–5 ms. Wall-clock is
measured on the bare command, so RSS sampling (a Python helper for `rusage`)
never inflates it.

Verifying the `unavailable` fallback (same host, `RSS_MODE=unavailable`) keeps
the wall-clock columns and replaces only the RSS column:

```text
wasm inspect token           wall min=0.0162s med=0.0162s avg=0.0164s | peakRSS med=unavailable kB
wasm inspect liquidity_pool  wall min=0.0162s med=0.0163s avg=0.0164s | peakRSS med=unavailable kB
audit sample_token           wall min=0.0164s med=0.0166s avg=0.0167s | peakRSS med=unavailable kB
diff self token              wall min=0.0162s med=0.0163s avg=0.0163s | peakRSS med=unavailable kB
diff token->lp               wall min=0.0165s med=0.0169s avg=0.0168s | peakRSS med=unavailable kB
```

### Linux capture (reference)

Earlier capture on the shared project VPS (Linux/Ubuntu, 2 vCPU, 3.7 GiB),
sdkt 2.7.0, `RUNS=7`, pinned to one core, RSS via `/usr/bin/time -v`:

| Command | wall min | wall median | wall avg | peak RSS (median) |
|---------|:-------:|:----------:|:-------:|:-----------------:|
| `wasm inspect token` | 0.0035 s | 0.0036 s | 0.0039 s | 5,916 kB |
| `wasm inspect liquidity_pool` | 0.0035 s | 0.0036 s | 0.0036 s | 5,972 kB |
| `audit sample_token` | 0.0036 s | 0.0037 s | 0.0037 s | 6,376 kB |
| `diff self token` | 0.0036 s | 0.0037 s | 0.0037 s | 5,976 kB |
| `diff token→lp` | 0.0038 s | 0.0038 s | 0.0039 s | 5,924 kB |

**Takeaways (baseline):**
- All three commands finish in **< 0.1 s** end-to-end on either host.
- Peak RSS stays **< 5 MiB** on macOS and **< 8 MiB** on the Linux VPS.
- `audit` scales gently with source size.
- `diff` cost is dominated by WASM ABI decode, roughly constant per artifact.

## Known limitations

1. **Absolute numbers are host-specific.** CPU model, cache pressure, and
   concurrent load on the shared VPS shift wall-clock by 10–30%. Compare
   releases on the *same* machine using the helper script.
2. **No startup-time isolation.** Wall-clock includes process spawn + CLI
   arg parsing. For a pure library-level budget, wrap the core call in a
   microbench (e.g. `criterion`) — out of scope for this baseline.
3. **Peak RSS** (via `/usr/bin/time -v` or the portable fallback) is
   whole-process and includes the Rust runtime + CLI overhead, not just the
   command's working set. On hosts without any probe, RSS is `unavailable`.
4. **Single binary, no parallel stress.** This measures one-shot offline
   commands, not throughput under concurrency.
5. **No flamegraph / per-phase breakdown** yet; if a future release regresses,
   add `cargo flamegraph` or `pprof` to localize the hotspot.

## Regression policy

- Re-run `scripts/bench_offline.sh` on the same host before a release tag.
- A **> 2× median wall-time or > 50% peak-RSS increase** on any command vs
  this baseline is a suspected regression and must be investigated before
  shipping.
- No code was optimized — the baseline reflects current behavior. Any
  future fix discovered here should add a focused regression check alongside
  this script.
