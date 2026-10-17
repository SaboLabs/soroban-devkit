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
# * Wall-clock measured with nanosecond `date`; peak RSS via GNU time -v
#   when available, otherwise a portable fallback (see below).
# * Reports min / median / average wall time and median peak RSS per command.
#
# RSS portability:
# * If GNU time (/usr/bin/time -v) is present, its "Maximum resident set size"
#   is used.
# * Otherwise, on Linux we poll /proc/<pid>/status VmHWM while the command runs.
# * Otherwise, on macOS we use `wait4` rusage via a tiny helper (see below).
# * If no portable probe exists (e.g. Windows), RSS is reported as
#   `unavailable` — never as an empty value or zero.
# * Single-thread pinning via `taskset 0x1` when available to reduce
# scheduler noise.
#
# Usage:
# SDKT=/path/to/sdkt WASM_DIR=/path/to/wasm AUDIT_SRC=/path/to/lib.rs \
# bash scripts/bench_offline.sh
#
# Dataset (3/4 fixtures):
#   WASM_DIR should contain token.wasm, atomic_swap.wasm, liquidity_pool.wasm,
#   timelock.wasm, single_offer.wasm (built from stellar/soroban-examples).
# AUDIT_SRC should be a real Soroban contract source (e.g. token/src/lib.rs).

set -uo pipefail

SDKT="${SDKT:-$(pwd)/target/release/sdkt}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WASM_DIR="${WASM_DIR:-$REPO_ROOT/scripts/fixtures/wasm}"
AUDIT_SRC="${AUDIT_SRC:-$REPO_ROOT/scripts/fixtures/token/src/lib.rs}"
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

# Detect GNU time (/usr/bin/time -v).
GNU_TIME=""
if [[ -x /usr/bin/time ]] && /usr/bin/time -v true >/dev/null 2>&1; then
  GNU_TIME="/usr/bin/time"
fi

# Portable RSS probe selection.
RSS_MODE="unavailable"
if [[ -n "$GNU_TIME" ]]; then
  RSS_MODE="gnu-time"
elif [[ -r /proc/self/status ]]; then
  RSS_MODE="proc"
elif command -v python3 >/dev/null 2>&1 && [[ "$(uname -s)" == "Darwin" ]]; then
  RSS_MODE="rusage"
fi

# Run a command and print peak RSS in kB (or "unavailable").
# Usage: measure_rss <cmd...>
measure_rss() {
  case "$RSS_MODE" in
    gnu-time)
      local out
      out=$($PIN "$GNU_TIME" -v "$@" 2>&1 >/dev/null)
      echo "$out" | awk -F': ' '/Maximum resident set size \(kbytes\)/{gsub(/ /,"",$2); print $2}'
      ;;
    proc)
      $PIN "$@" >/dev/null 2>&1 &
      local pid=$! peak=0
      while kill -0 "$pid" 2>/dev/null; do
        local v
        v=$(awk '/VmHWM/{print $2}' "/proc/$pid/status" 2>/dev/null)
        [[ -n "$v" && "$v" -gt "$peak" ]] && peak=$v
        sleep 0.01
      done
      wait "$pid" 2>/dev/null || true
      echo "$peak"
      ;;
    rusage)
      python3 - "$@" <<'PY'
import resource, subprocess, sys
p = subprocess.run(sys.argv[1:], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
r = resource.getrusage(resource.RUSAGE_CHILDREN)
print(r.ru_maxrss)  # bytes on macOS
PY
      ;;
    *)
      $PIN "$@" >/dev/null 2>&1 || true
      echo "unavailable"
      ;;
  esac
}

# Collect N wall times (seconds, float) into an array via stdout capture.
bench_time() {
 # $1 = label, rest = command
  local label="$1"; shift
  local times=() rss=() i wall rss_kb
 # warmup
  $PIN "$@" >/dev/null 2>&1 || true
  for ((i=0;i<RUNS;i++)); do
    local t0 t1
    t0=$(date +%s.%N)
    rss_kb=$(measure_rss "$@")
    t1=$(date +%s.%N)
    wall=$(awk "BEGIN{printf \"%.4f\", $t1-$t0}")
    times+=("$wall")
    rss+=("${rss_kb:-unavailable}")
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
  if [[ "${rss[0]}" == "unavailable" ]]; then
    rss_med="unavailable"
  else
    rss_med=$(printf '%s\n' "${rss[@]}" | sort -n | awk '{a[NR]=$1} END{print (NR%2)? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2}')
  fi
  printf '%-28s wall min=%.4fs med=%.4fs avg=%.4fs | peakRSS med=%s kB\n' \
         "$label" "$min" "$med" "$avg" "$rss_med"
}

echo "=== Offline benchmark :: sdkt=$( "$SDKT" --version 2>/dev/null || echo unknown ) ==="
echo "RUNS=$RUNS  WASM_DIR=$WASM_DIR  AUDIT_SRC=$AUDIT_SRC"
echo "pin=${PIN:-none}  rss=${RSS_MODE}"
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
