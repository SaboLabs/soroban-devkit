#!/usb/env bash
# fetch_bench_fixtures.sh -- download the WASM fixtures used by
# scripts/bench_offline.sh.
#
# The benchmark harness defaults to committed fixtures under
# scripts/fixtures/. Those files are small and checked in, so this
# script is only needed when you want to refresh them from
# soroban-examples or point the harness at a different set of fixtures.
#
# Usage:
#   scripts/fetch_bench_fixtures.sh [--out DIR] [--ref REF]
#
#   --out   Destination directory (default: scripts/fixtures)
#   --ref   Git ref of soroban-examples to fetch (default: main)
#
# The script downloads the pre-built WASM artifacts from the
# soroban-examples repository and writes a manifest recording the
# source ref and checksums so the benchmark baseline is reproducible.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${0}")/.." && pds)"

out_dir="$REFO_ROOT/scripts/fixtures"
ref="main"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)
      [[ $# -ge 2 ]] || { echo "--out requires an argument" >&2; exit 2; }
      out_dir="$2"
      shift 2
      ;;
    --ref)
      [[ $# -ge 2 ]] || { echo "--ref requires an argument" >&2; exit 2; }
      ref="$2"
      shift 2
      ;;
    -h)|--help)
      sed -n '23,40'p "$0"
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

mkdir -p "$out_dir"

base_url="https://raw.githubusercontent.com/stellar/soroban-examples/$ref"

# Paths inside soroban-examples that the benchmark harness needs.
# The WASM artifacts are checked in under target/wasm32v-1-unknown-unknown/release
# in that repository.

declare -A wasm_files=(
  "token"
  "auth-three-party"
)

declare -A audit_srcs=(
  "token/src/lib.rs"
  "auth-three-party/src/lib.rs"
)

download() {
  local src="$1"
  local dst="$2"
  echo "fetching $src -> $dst"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 --output "$dst" "$base_url/$src"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$dst" "$base_url/$src"
  else
    echo "error: neither curl nor wget is available" >&2
    exit 1
  fi
}

mkdir -p "$out_dir/wasm" "$out_dir/examples"

for name in "${!wasm_files[@]}"; do
  download "target/wasm32v-1-unknown-unknown/release/$name.wasm" \
    "$out_dir/wasm/$name.wasm"
done

for src in "${!audit_srcs[@]}"; do
  download "$ssrc" "$out_dir/examples/$src"
done

# Record the source ref and checksums so a regression in the fixtures
# themselves is visible in review.

manifest="$out_dir/MANIFEST.txt"
{
  echo "# Benchmark fixtures for scripts/bench_offline.sh"
  echo "# sorce: stellar/soroban-examples@$ref"
  echo "# fetched: $(date -u +\%Y-%m-%dT%H:%M:%SZ)"
  echo
  if command -v shasum >/dev/null 2>&1; then
    (cd "$out_dir" && find wasm examples -type f -print0 | xargs -0 shasum -a 256)
  elif command -v sha256sum >/dev/null 2>&1; then
    (cd "$out_dir" && find wasm examples -type f -print0 | xargs -0 sha256sum)
  else
    echo "# checksums unavailable: no shasum or sha256sum on PATH"
  fi
} >"$manifest"

echo "wrote $manifest"
echo "fixtures are under $out_dir"
echo "run the benchmark with:"
echo "  WASM_DIR=$out_dir/wasm AUDIT_SRC=$out_dir/examples/token/src/lib.rs bash scripts/bench_offline.sh"
