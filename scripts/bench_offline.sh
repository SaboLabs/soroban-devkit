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
# * Wall-clock measured with a portable clock:
#     - nanosecond `date +%s.%N` where supported (GNU/Linux),
#     - otherwise Perl/Python high-resolution time, or whole seconds as a
#       last resort (macOS BSD `date` lacks %N).
# * Peak RSS probe, in order of preference:
#     1. GNU time (`/usr/bin/time -v`) when present (Linux, most CI images).
#     2. BSD time (`/usr/bin/time -l`, macOS) reporting max RSS in bytes.
#     3. Otherwise RSS is reported as `unavailable` (never as 0).
# * Reports min / median / average wall time and median peak RSS per command.
# * Single-thread pinning via `taskset 0x1` when available to reduce
#   scheduler noise.
#
# Usage:
#   SDKT=/path/to/sdkt WASM_DIR=/path/to/wasm AUDIT_SRC=/path/to/lib.rs \
#     bash scripts/bench_offline.sh
#
# Fixtures: WASM_DIR and AUDIT_SRC must point at a fixture set. The default
# location is bench/fixtures/ under the repo root; if it is missing, the script
# fails with an explicit message instead of silently benchmarking nothing.
#
# Output format (one line per command):
#   <label> wall min=..s med=..s avg=..s | peakRSS med=<N> kB
# where <N> is a number, or the literal `unavailable` when no RSS probe runs.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

SDKT="${SDKT:-$REPO_ROOT/target/release/sdkt}"
WASM_DIR="${WASM_DIR:-$REPO_ROOT/bench/fixtures/wasm}"
AUDIT_SRC="${AUDIT_SRC:-$REPO_ROOT/bench/fixtures/examples/token/src/lib.rs}"
RUNS="${RUNS:-7}"
WARMUP=1

if [[ ! -x "$SDKT" ]]; then
  echo "ERROR: sdkt binary not found/executable at: $SDKT" >&2
  exit 1
fi

if [[ ! -f "$WASM_DIR/token.wasm" || ! -f "$WASM_DIR/liquidity_pool.wasm" || ! -f "$AUDIT_SRC" ]]; then
  echo "ERROR: benchmark fixtures missing (WASM_DIR=$WASM_DIR, AUDIT_SRC=$AUDIT_SRC)." >&2
  echo "Provide them via WASM_DIR / AUDIT_SRC overrides, or add them under bench/fixtures/." >&2
  exit 1
fi

# Detect RSS probe once.
RSS_MODE="none"
GNU_TIME="/usr/bin/time"
if [[ -x "$GNU_TIME" ]] && "$GNU_TIME" -v true >/dev/null 2>&1; then
  RSS_MODE="gnu"
elif [[ -x "$GNU_TIME" ]] && "$GNU_TIME" -l true >/dev/null 2>&1; then
  RSS_MODE="bsd"
fi

# Pick a high-resolution clock once.
CLOCK_MODE="seconds"
if date +%s.%N 2>/dev/null | grep -q '\.[0-9]'; then
  CLOCK_MODE="date"
elif command -v python3 >/dev/null 2>&1; then
  CLOCK_MODE="python"
elif command -v perl >/dev/null 2>&1; then
  CLOCK_MODE="perl"
fi

now() {
  case "$CLOCK_MODE" in
    date)   date +%s.%N ;;
    python) python3 -c 'import time; print(f"{time.time():.6f}")' ;;
    perl)   perl -MTime::HiRes=time -e 'printf "%.6f\n", time' ;;
    *)      date +%s ;;
  esac
}

# Optional single-core pinning for steadier numbers.
PIN=""
if command -v taskset >/dev/null 2>&1; then
  PIN="taskset 0x1"
fi

# Median of newline-separated numbers on stdin.
median() {
  sort -n | awk '{a[NR]=$1} END{ if (NR==0) {print "0"} else if (NR%2) {print a[(NR+1)/2]} else {printf "%.4f\n", (a[NR/2]+a[NR/2+1])/2} }'
}

# Run one measured invocation; prints "<wall_seconds> <rss_kb_or_NA>".
measure_once() {
  local t0 t1 out rss_kb="NA"
  t0=$(now)
  case "$RSS_MODE" in
    gnu)
      out=$($PIN "$GNU_TIME" -v "$@" 2>&1 >/dev/null)
      rss_kb=$(echo "$out" | awk -F': ' '/Maximum resident set size \(kbytes\)/{gsub(/ /,"",$2); print $2}')
      ;;
    bsd)
      out=$($PIN "$GNU_TIME" -l "$@" 2>&1 >/dev/null)
      # macOS reports bytes; convert to kB.
      local bytes
      bytes=$(echo "$out" | awk '/maximum resident set size/{print $1}')
      if [[ -n "$bytes" ]]; then rss_kb=$(( bytes / 1024 )); fi
      ;;
    *)
      $PIN "$@" >/dev/null 2>&1
      ;;
  esac
  t1=$(now)
  awk "BEGIN{printf \"%.4f %s\n\", $t1-$t0, \"${rss_kb:-NA}\"}"
}

bench_time() {
  local label="$1"; shift
  local times=() rss=() i line wall rss_v
  # warmup (discarded)
  for ((i=0;i<WARMUP;i++)); do measure_once "$@" >/dev/null; done
  for ((i=0;i<RUNS;i++)); do
    line=$(measure_once "$@")
    wall=${line%% *}
    rss_v=${line##* }
    times+=("$wall")
    rss+=("$rss_v")
  done

  local sorted min med sum avg rss_med rss_str
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  min=$(echo "$sorted" | head -1)
  med=$(printf '%s\n' "${times[@]}" | median)
  sum=$(printf '%s\n' "${times[@]}" | awk '{s+=$1} END{printf "%.4f", s}')
  avg=$(awk "BEGIN{printf \"%.4f\", $sum/$RUNS}")

  if [[ "$RSS_MODE" == "none" ]] || printf '%s\n' "${rss[@]}" | grep -q NA; then
    rss_str="unavailable"
  else
    rss_med=$(printf '%s\n' "${rss[@]}" | median)
    rss_str="$rss_med kB"
  fi

  printf '%-28s wall min=%.4fs med=%.4fs avg=%.4fs | peakRSS med=%s\n' \
         "$label" "$min" "$med" "$avg" "$rss_str"
}

echo "=== Offline benchmark :: sdkt=$( "$SDKT" --version 2>/dev/null || echo unknown ) ==="
echo "RUNS=$RUNS  WASM_DIR=$WASM_DIR  AUDIT_SRC=$AUDIT_SRC"
echo "pin=${PIN:-none}  rss_probe=$RSS_MODE  clock=$CLOCK_MODE"
echo

TOKEN="$WASM_DIR/token.wasm"
LP="$WASM_DIR/liquidity_pool.wasm"

bench_time "wasm inspect token"          "$SDKT" wasm inspect "$TOKEN"
bench_time "wasm inspect liquidity_pool" "$SDKT" wasm inspect "$LP"
bench_time "audit token/src/lib.rs"      "$SDKT" audit "$AUDIT_SRC"
bench_time "diff self token"             "$SDKT" diff --old-wasm "$TOKEN" --new-wasm "$TOKEN" --upgrade-safety
bench_time "diff token->lp"              "$SDKT" diff --old-wasm "$TOKEN" --new-wasm "$LP" --upgrade-safety

echo
echo "Done."
