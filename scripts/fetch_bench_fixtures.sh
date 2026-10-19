#!/usr/bin/env bash
# fetch_bench_fixtures.sh -- (re)build the WASM fixtures used by
# scripts/bench_offline.sh.
#
# stellar/soroban-examples does not publish pre-built WASM artifacts (the
# upstream `target/` directory is gitignored and the GitHub releases carry no
# assets), so this script fetches the contract *sources* at a pinned commit and
# builds them locally with `stellar contract build`.
#
# The built files are written to scripts/fixtures/wasm/ with the names the
# benchmark harness expects, and a manifest records the source ref plus
# checksums so a fixture change is visible in review.
#
# Requirements:
#   * curl or wget
#   * tar
#   * stellar-cli (v25.2.0+, `stellar contract build`)
#   * a Rust toolchain with the wasm32v1-none target
#
# Usage:
#   scripts/fetch_bench_fixtures.sh [--out DIR] [--ref REF]
#
#   --out   Destination directory (default: scripts/fixtures)
#   --ref   Commit or tag of soroban-examples to build (default: the pinned
#           commit recorded in this script)
set -euo pipefail

# Pinned so a re-run reproduces the committed fixtures byte-for-byte.
DEFAULT_REF="03d42aa6b973dcf3a453a99d0c6a6e8d25a196e2"

usage() {
  cat <<'EOF'
Usage:
  scripts/fetch_bench_fixtures.sh [--out DIR] [--ref REF]

  --out   Destination directory (default: scripts/fixtures)
  --ref   Commit or tag of soroban-examples to build
          (default: the pinned commit recorded in this script)

Requires curl or wget, tar, stellar-cli (v25.2.0+) and a Rust toolchain
with the wasm32v1-none target.
EOF
}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="$REPO_ROOT/scripts/fixtures"
ref="$DEFAULT_REF"

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
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if ! command -v stellar >/dev/null 2>&1; then
  echo "error: stellar-cli is required (v25.2.0+): https://developers.stellar.org/docs/tools/developer-tools/cli" >&2
  exit 1
fi

# Fixture name -> crate directory inside soroban-examples.
wasm_files=(
  "token:token"
  "liquidity_pool:liquidity_pool"
  "atomic_swap:atomic_swap"
  "timelock:timelock"
)

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/bench-fixtures.XXXXXX")"
cleanup() { rm -rf "$work_dir"; }
trap cleanup EXIT

src_url="https://codeload.github.com/stellar/soroban-examples/tar.gz/$ref"
tarball="$work_dir/soroban-examples.tar.gz"

echo "fetching soroban-examples@$ref"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL --retry 3 --output "$tarball" "$src_url"
elif command -v wget >/dev/null 2>&1; then
  wget -q -O "$tarball" "$src_url"
else
  echo "error: neither curl nor wget is available" >&2
  exit 1
fi

mkdir -p "$work_dir/src"
tar -xzf "$tarball" -C "$work_dir/src" --strip-components=1

mkdir -p "$out_dir/wasm"

for entry in "${wasm_files[@]}"; do
  name="${entry%%:*}"
  crate="${entry#*:}"
  echo "building $crate -> $name.wasm"
  ( cd "$work_dir/src/$crate" && stellar contract build >/dev/null )
  built="$(find "$work_dir/src/$crate/target/wasm32v1-none/release" -maxdepth 1 -name '*.wasm' \
    -not -name '*_test.wasm' | head -n 1)"
  if [[ -z "$built" ]]; then
    echo "error: no wasm produced for $crate" >&2
    exit 1
  fi
  cp "$built" "$out_dir/wasm/$name.wasm"
done

# Record the source ref and checksums so a regression in the fixtures
# themselves is visible in review.
manifest="$out_dir/MANIFEST.txt"
{
  echo "# Benchmark WASM fixtures for scripts/bench_offline.sh"
  echo "# source: stellar/soroban-examples@$ref"
  echo "# built with: stellar contract build (wasm32v1-none, release)"
  echo "# fetched: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo
  if command -v shasum >/dev/null 2>&1; then
    (cd "$out_dir" && find wasm -type f -print0 | xargs -0 shasum -a 256)
  elif command -v sha256sum >/dev/null 2>&1; then
    (cd "$out_dir" && find wasm -type f -print0 | xargs -0 sha256sum)
  else
    echo "# checksums unavailable: no shasum or sha256sum on PATH"
  fi
} >"$manifest"

echo "wrote $manifest"
echo "fixtures are under $out_dir/wasm"
echo "run the benchmark with:"
echo "  bash scripts/bench_offline.sh"
