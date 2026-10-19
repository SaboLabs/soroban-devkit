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
# * Otherwise, on macOS we use `getrusage(RUSAGE_CHILDREN)` via a small
#   Python helper and convert the reported bytes to kB.
# * If no portable probe exists (e.g. Windows), RSS is reported as
#   `unavailable` — never as an empty value or zero.
# * Single-thread pinning via `taskset 0x1` when available to reduce
#   scheduler noise.
#
# Usage:
# SDKT=/path/to/sdkt WASM_DIR=/path/to/wasm AUDIT_SRC=/path/to/lib.rs \
# bash scripts/bench_offline.sh
#
# Fixtures:
#   WASM_DIR defaults to the committed fixtures in scripts/fixtures/wasm
#   (token.wasm, liquidity_pool.wasm, atomic_swap.wasm, timelock.wasm, built
#   from stellar/soroban-examples). AUDIT_SRC defaults to the self-contained
#   sample contract in examples/sample_token/src/lib.rs.
#   To refresh the WASM fixtures, run scripts/fetch_bench_fixtures.sh.

set -uo pipefail

SDKT="${SDKT:-$(pwd)/target/release/sdkt}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WASM_DIR="${WASM_DIR:-$REPO_ROOT/scripts/fixtures/wasm}"
AUDIT_SRC="${AUDIT_SRC:-$REPO_ROOT/examples/sample_token/src/lib.rs}"
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

# Detect GNU time (`time -v`). `type -P` skips the shell builtin.
GNU_TIME=""
if candidate_time="$(type -P time 2>/dev/null)" && [[ -n "$candidate_time" ]]; then
  if "$candidate_time" -v true >/dev/null 2>&1; then
    GNU_TIME="$candidate_time"
  fi
fi

# Portable RSS probe selection. RSS_MODE may be set explicitly (useful to
# exercise the `unavailable` path in docs/verification).
if [[ -z "${RSS_MODE:-}" ]]; then
  RSS_MODE="unavailable"
  if [[ -n "$GNU_TIME" ]]; then
    RSS_MODE="gnu-time"
  elif [[ -r /proc/self/status ]]; then
    RSS_MODE="proc"
  elif command -v python3 >/dev/null 2>&1 && [[ "$(uname -s)" == "Darwin" ]]; then
    RSS_MODE="rusage"
  fi
fi

case "$RSS_MODE" in
  gnu-time|proc|rusage|unavailable) ;;
  *)
    echo "ERROR: RSS_MODE must be one of gnu-time, proc, rusage, unavailable" >&2
    exit 2
    ;;
esac

# Run a command and print peak RSS in kB (or "unavailable").
# Usage: measure_rss <cmd...>
measure_rss() {
  case "$RSS_MODE" in
    gnu-time)
      local out rss
      out=$($PIN "$GNU_TIME" -v "$@" 2>&1 >/dev/null)
      rss=$(echo "$out" | awk -F': ' '/Maximum resident set size \(kbytes\)/{gsub(/ /,"",$2); print $2}')
      # A short-lived process can exit before the kernel reports the high-water
      # mark, in which case we have no measurement rather than a zero.
      echo "${rss:-unavailable}"
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
      # The process can finish before the first poll: report "unavailable"
      # rather than a misleading 0 kB.
      if [[ "$peak" -gt 0 ]]; then echo "$peak"; else echo "unavailable"; fi
      ;;
    rusage)
      python3 - "$@" <<'PY'
import resource, subprocess, sys
subprocess.run(sys.argv[1:], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
r = resource.getrusage(resource.RUSAGE_CHILDREN)
if r.ru_maxrss > 0:
    print(r.ru_maxrss // 1024)  # macOS reports bytes; the harness labels kB
else:
    print("unavailable")
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
  local times=() rss=() i wall w
  # warmup (discarded)
  for ((w=0;w<WARMUP;w++)); do
    $PIN "$@" >/dev/null 2>&1 || true
  done
  for ((i=0;i<RUNS;i++)); do
    local t0 t1
    # Wall-clock is measured on the bare command so sampling overhead (a
    # Python helper for `rusage`, a polling loop for `/proc`) never inflates it.
    t0=$(date +%s.%N)
    $PIN "$@" >/dev/null 2>&1 || true
    t1=$(date +%s.%N)
    wall=$(awk "BEGIN{printf \"%.4f\", $t1-$t0}")
    times+=("$wall")
    rss+=("$(measure_rss "$@")")
  done
 # stats: sort, median, avg
  local sorted
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  local min avg med sum=0
  min=$(echo "$sorted" | head -1)
  med=$(echo "$sorted" | awk '{a[NR]=$1} END{print (NR%2)? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2}')
  sum=$(printf '%s\n' "${times[@]}" | awk '{s+=$1} END{printf "%.4f", s}')
  avg=$(awk "BEGIN{printf \"%.4f\", $sum/$RUNS}")
  # A median over a mixed numeric/unavailable sample would be meaningless, so
  # any missing sample makes the whole column "unavailable".
  local rss_med sample
  rss_med=""
  for sample in "${rss[@]}"; do
    if [[ "$sample" == "unavailable" ]]; then
      rss_med="unavailable"
      break
    fi
  done
  if [[ -z "$rss_med" ]]; then
    rss_med=$(printf '%s\n' "${rss[@]}" | sort -n | awk '{a[NR]=$1} END{print (NR%2)? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2}')
  fi
  printf '%-28s wall min=%.4fs med=%.4fs avg=%.4fs | peakRSS med=%s kB\n' \
         "$label" "$min" "$med" "$avg" "$rss_med"
}

echo "=== Offline benchmark :: sdkt=$( "$SDKT" --version 2>/dev/null || echo unknown ) ==="
echo "RUNS=$RUNS  WASM_DIR=$WASM_DIR  AUDIT_SRC=$AUDIT_SRC"
echo "pin=${PIN:-none}"
echo "rss=${RSS_MODE}"
echo

TOKEN="$WASM_DIR/token.wasm"
LP="$WASM_DIR/liquidity_pool.wasm"

missing=0
for f in "$TOKEN" "$LP" "$AUDIT_SRC"; do
  if [[ ! -f "$f" ]]; then
    echo "ERROR: fixture not found: $f" >&2
    missing=1
  fi
done
if [[ "$missing" -ne 0 ]]; then
  echo "Run scripts/fetch_bench_fixtures.sh to (re)build the fixtures, or point" >&2
  echo "WASM_DIR/AUDIT_SRC at an existing set." >&2
  exit 1
fi

bench_time "wasm inspect token"      "$SDKT" wasm inspect "$TOKEN"
bench_time "wasm inspect liquidity_pool" "$SDKT" wasm inspect "$LP"
bench_time "audit sample_token"      "$SDKT" audit "$AUDIT_SRC"
bench_time "diff self token"         "$SDKT" diff --old-wasm "$TOKEN" --new-wasm "$TOKEN" --upgrade-safety
bench_time "diff token->lp"          "$SDKT" diff --old-wasm "$TOKEN" --new-wasm "$LP" --upgrade-safety

echo
echo "Done."
