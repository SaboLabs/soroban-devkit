#!/usr/bin/env bash
# — offline sdkt performance benchmark (reproducible).
#
# Measures wall-clock time and peak resident set size (RSS) for the three
# most important offline sdkt commands:
# - sdkt wasm inspect <wasm>
# - sdkt audit <src.rs>
# - sdkt diff --upgrade-safety --old-wasm A --new-wasm B
#
# Methodology to avoid flaky numbers:
# * A release build of sdkt is used (debug builds are not representative).
# * Each command is run WARMUP (1, discarded) + RUNS (default 7) times.
# * Wall-clock measured with nanosecond `date`.
# * Peak RSS probe, in order of preference:
#     1. GNU time (`/usr/bin/time -v`) when it exists and reports RSS.
#     2. Linux /proc polling of VmHWM (peak RSS high-water mark) of the child.
#     3. Otherwise RSS is reported as `unavailable` (never as 0).
# * Reports min / median / average wall time and median peak RSS per command.
# * Single-thread pinning via `taskset 0x1` when available to reduce
#   scheduler noise.
#
# Usage:
#   SDKT=/path/to/sdkt WASM_DIR=/path/to/wasm AUDIT_SRC=/path/to/lib.rs \
#     bash scripts/bench_offline.sh
#
# Default fixtures live in scripts/bench_fixtures/ (committed). Run
# scripts/fetch_bench_fixtures.sh to refresh them from upstream sources.
#
# Dataset (5 fixtures):
#   WASM_DIR should contain token.wasm, atomic_swap.wasm, liquidity_pool.wasm,
#   timelock.wasm, single_offer.wasm (built from stellar/soroban-examples).
# AUDIT_SRC should be a real Soroban contract source (e.g. token/src/lib.rs).

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

SDKT="${SDKT:-$REPO_ROOT/target/release/sdkt}"
WASM_DIR="${WASM_DIR:-$SCRIPT_DIR/bench_fixtures/wasm}"
AUDIT_SRC="${AUDIT_SRC:-$SCRIPT_DIR/bench_fixtures/src/token/lib.rs}"
RUNS="${RUNS:-7}"
WARMUP=1

if [[ ! -x "$SDKT" ]]; then
  echo "ERROR: sdkt binary not found/executable at: $SDKT" >&2
  exit 1
fi
for f in "$WASM_DIR/token.wasm" "$WASM_DIR/liquidity_pool.wasm" "$AUDIT_SRC"; do
  if [[ ! -f "$f" ]]; then
    echo "ERROR: missing benchmark fixture: $f (run scripts/fetch_bench_fixtures.sh)" >&2
    exit 1
  fi
done

# Optional single-core pinning for steadier numbers.
PIN=""
if command -v taskset >/dev/null 2>&1; then
  PIN="taskset 0x1"
fi

# Detect the RSS probe once.
#   gnu-time : /usr/bin/time -v exists and prints a max RSS line
#   proc     : Linux /proc/<pid>/status VmHWM polling
#   none     : no portable probe -> report "unavailable"
RSS_PROBE="none"
GNU_TIME=""
if [[ -x /usr/bin/time ]] && /usr/bin/time -v true >/dev/null 2>&1; then
  GNU_TIME="/usr/bin/time"
  RSS_PROBE="gnu-time"
elif [[ -r /proc/self/status ]]; then
  RSS_PROBE="proc"
fi

# Run a command, print its peak RSS in kB (or "unavailable") on stdout's
# last line via the global LAST_RSS. Wall time is measured by the caller.
LAST_RSS="unavailable"
run_with_rss() {
  LAST_RSS="unavailable"
  case "$RSS_PROBE" in
    gnu-time)
      local out
      out=$($PIN "$GNU_TIME" -v "$@" 2>&1 >/dev/null)
      local v
      v=$(echo "$out" | awk -F': ' '/Maximum resident set size \(kbytes\)/{gsub(/ /,"",$2); print $2}')
      if [[ -n "$v" ]]; then LAST_RSS="$v"; fi
      ;;
    proc)
      $PIN "$@" >/dev/null 2>&1 &
      local pid=$! peak=0 hwm
      while kill -0 "$pid" 2>/dev/null; do
        hwm=$(awk '/^VmHWM:/{print $2}' "/proc/$pid/status" 2>/dev/null)
        if [[ -n "$hwm" ]] && (( hwm > peak )); then peak=$hwm; fi
        sleep 0.005
      done
      wait "$pid" 2>/dev/null
      if (( peak > 0 )); then LAST_RSS="$peak"; fi
      ;;
    *)
      $PIN "$@" >/dev/null 2>&1
      ;;
  esac
}

median() {
  awk '{a[NR]=$1} END{ if (NR==0) {print "unavailable"; exit}
    print (NR%2)? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2 }'
}

# Collect N wall times (seconds, float) and RSS into arrays.
bench_time() {
  # $1 = label, rest = command
  local label="$1"; shift
  local times=() rss=() i wall
  # warmup
  $PIN "$@" >/dev/null 2>&1 || true
  for ((i=0;i<RUNS;i++)); do
    local t0 t1
    t0=$(date +%s.%N)
    run_with_rss "$@"
    t1=$(date +%s.%N)
    wall=$(awk "BEGIN{printf \"%.4f\", $t1-$t0}")
    times+=("$wall")
    rss+=("$LAST_RSS")
  done
  local sorted min med sum avg rss_med
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  min=$(echo "$sorted" | head -1)
  med=$(echo "$sorted" | median)
  sum=$(printf '%s\n' "${times[@]}" | awk '{s+=$1} END{printf "%.4f", s}')
  avg=$(awk "BEGIN{printf \"%.4f\", $sum/$RUNS}")
  # RSS: if any run was unavailable, report unavailable (never fake zero).
  if printf '%s\n' "${rss[@]}" | grep -q '^unavailable$'; then
    rss_med="unavailable"
  else
    rss_med=$(printf '%s\n' "${rss[@]}" | sort -n | median)
  fi
  printf '%-28s wall min=%.4fs med=%.4fs avg=%.4fs | peakRSS med=%s kB\n' \
         "$label" "$min" "$med" "$avg" "$rss_med"
}

echo "=== Offline benchmark :: sdkt=$( "$SDKT" --version 2>/dev/null || echo unknown ) ==="
echo "RUNS=$RUNS  WASM_DIR=$WASM_DIR  AUDIT_SRC=$AUDIT_SRC"
echo "pin=${PIN:-none}  rss_probe=$RSS_PROBE"
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
