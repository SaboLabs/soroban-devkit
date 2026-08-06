#!/usr/bin/env bash
# M35 — offline sdkt performance benchmark (reproducible).
#
# Measures wall-clock time and peak resident set size (RSS) for the three
# most important offline sdkt commands:
#   - sdkt wasm inspect <wasm>
#   - sdkt audit <src.rs>
#   - sdkt diff --upgrade-safety --old-wasm A --new-wasm B
#
# Methodology to avoid flaky numbers:
#   * A release build of sdkt is used (debug builds are not representative).
#   * Each command is run WARMUP (1, discarded) + RUNS (default 7) times.
#   * Wall-clock measured with nanosecond `date`; peak RSS via /usr/bin/time -v.
#   * Reports min / median / average wall time and median peak RSS per command.
#   * Single-thread pinning via `taskset 0x1` when available to reduce
#     scheduler noise.
#
# Usage:
#   SDKT=/path/to/sdkt WASM_DIR=/path/to/wasm AUDIT_SRC=/path/to/lib.rs \
#     bash scripts/bench_offline.sh
#
# Dataset (M33/M34 fixtures):
#   WASM_DIR should contain token.wasm, atomic_swap.wasm, liquidity_pool.wasm,
#   timelock.wasm, single_offer.wasm (built from stellar/soroban-examples).
#   AUDIT_SRC should be a real Soroban contract source (e.g. token/src/lib.rs).

set -uo pipefail

SDKT="${SDKT:-$(pwd)/target/release/sdkt}"
WASM_DIR="${WASM_DIR:-/tmp/m33/wasm}"
AUDIT_SRC="${AUDIT_SRC:-/tmp/m33/examples/token/src/lib.rs}"
RUNS="${RUNS:-7}"
WARMUP=1

if [[ ! -x "$SDKT" ]]; then
  echo "ERROR: sdkt binary not found/executable at: $SDKT" >&2
  exit 1
fi

# Optional single-core pinning for steadier numbers.
PIN=""
if command -v taskset >/dev/null 2>&1; then
  PIN="taskset 0x1"
fi

# Collect N wall times (seconds, float) into an array via stdout capture.
bench_time() {
  # $1 = label, rest = command
  local label="$1"; shift
  local times=() rss=() i out wall rss_kb
  # warmup
  $PIN "$@" >/dev/null 2>&1 || true
  for ((i=0;i<RUNS;i++)); do
    local t0 t1
    t0=$(date +%s.%N)
    out=$($PIN /usr/bin/time -v "$@" 2>&1 >/dev/null)
    t1=$(date +%s.%N)
    wall=$(awk "BEGIN{printf \"%.4f\", $t1-$t0}")
    rss_kb=$(echo "$out" | awk -F': ' '/Maximum resident set size \(kbytes\)/{gsub(/ /,"",$2); print $2}')
    times+=("$wall")
    rss+=("${rss_kb:-0}")
  done
  # stats: sort, median, avg
  local sorted
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  local min avg med sum=0
  min=$(echo "$sorted" | head -1)
  med=$(echo "$sorted" | awk '{a[NR]=$1} END{print (NR%2)? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2}')
  sum=$(echo "${times[@]}" | tr ' ' '\n' | awk '{s+=$1} END{printf "%.4f", s}')
  avg=$(awk "BEGIN{printf \"%.4f\", $sum/$RUNS}")
  local rss_med
  rss_med=$(printf '%s\n' "${rss[@]}" | sort -n | awk '{a[NR]=$1} END{print (NR%2)? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2}')
  printf '%-28s wall min=%.4fs med=%.4fs avg=%.4fs | peakRSS med=%s kB\n' \
         "$label" "$min" "$med" "$avg" "$rss_med"
}

echo "=== M35 offline benchmark :: sdkt=$( "$SDKT" --version 2>/dev/null || echo unknown ) ==="
echo "RUNS=$RUNS  WASM_DIR=$WASM_DIR  AUDIT_SRC=$AUDIT_SRC"
echo "pin=${PIN:-none}"
echo

TOKEN="$WASM_DIR/token.wasm"
LP="$WASM_DIR/liquidity_pool.wasm"

bench_time "wasm inspect token"      "$SDKT" wasm inspect "$TOKEN"
bench_time "wasm inspect liquidity_pool" "$SDKT" wasm inspect "$LP"
bench_time "audit token/src/lib.rs"  "$SDKT" audit "$AUDIT_SRC"
bench_time "diff self token"         "$SDKT" diff --old-wasm "$TOKEN" --new-wasm "$TOKEN" --upgrade-safety
bench_time "diff token->lp"          "$SDKT" diff --old-wasm "$TOKEN" --new-wasm "$LP" --upgrade-safety

echo
echo "Done."
