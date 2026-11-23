#!/usr/bin/env bash
# Build the Web Playground's browser WASM runtime.
#
# The playground glue crate is intentionally excluded from the native workspace
# (see root Cargo.toml), so it is compiled explicitly for wasm32-unknown-unknown
# here. Output lands in website/playground/wasm/ and is served as a static
# asset alongside the playground page — no build step at deploy time.
#
# Requirements:
# rustup toolchain install 1.88.0 --target wasm32-unknown-unknown
# cargo +1.88.0 install wasm-bindgen-cli --version 0.2.127 --locked
# macOS only: brew install llvm (Apple Clang has no wasm32 backend)
#
# Usage: bash website/playground/build.sh
set -euo pipefail

cd "$(dirname "$0")/../.."   # repo root (website/playground -> website -> repo)

RUST_TOOLCHAIN="1.88.0"
WASM_BINDGEN_VERSION="0.2.127"
OUT_DIR="${PLAYGROUND_WASM_OUT:-website/playground/wasm}"

# zstd-sys compiles C sources for the target. Apple Clang does not ship a
# wasm32 backend, while Homebrew LLVM does. Prefer it automatically on macOS so
# the documented build command works without caller-specific environment vars.
if [[ "$(uname -s)" == "Darwin" ]]; then
  LLVM_PREFIX=""
  if command -v brew >/dev/null 2>&1; then
    LLVM_PREFIX="$(brew --prefix llvm 2>/dev/null || true)"
  fi
  if [[ -z "${LLVM_PREFIX}" ]] || [[ ! -x "${LLVM_PREFIX}/bin/clang" ]]; then
    if [[ -x /opt/homebrew/opt/llvm/bin/clang ]]; then
      LLVM_PREFIX="/opt/homebrew/opt/llvm"
    elif [[ -x /usr/local/opt/llvm/bin/clang ]]; then
      LLVM_PREFIX="/usr/local/opt/llvm"
    fi
  fi
  if [[ -n "${LLVM_PREFIX}" ]] && [[ -x "${LLVM_PREFIX}/bin/clang" ]]; then
    export CC_wasm32_unknown_unknown="${CC_wasm32_unknown_unknown:-${LLVM_PREFIX}/bin/clang}"
    export AR_wasm32_unknown_unknown="${AR_wasm32_unknown_unknown:-${LLVM_PREFIX}/bin/llvm-ar}"
  fi
fi

echo "==> Building sdkt-playground for wasm32-unknown-unknown (release)"
cargo +"${RUST_TOOLCHAIN}" build -p sdkt-playground --locked --release \
  --target wasm32-unknown-unknown \
  --manifest-path crates/sdkt-playground/Cargo.toml

echo "==> Generating JS shim + .wasm via wasm-bindgen ${WASM_BINDGEN_VERSION}"
mkdir -p "${OUT_DIR}"

# Target dir lives inside the crate because it is a standalone (non-workspace)
# crate rooted at crates/sdkt-playground/.
TARGET_DIR="crates/sdkt-playground/target/wasm32-unknown-unknown/release/sdkt_playground.wasm"
if [ ! -f "${TARGET_DIR}" ]; then
  echo "error: expected build output at ${TARGET_DIR}" >&2
  exit 1
fi

wasm-bindgen \
  --target web \
  --no-typescript \
  --out-dir "${OUT_DIR}" \
  "${TARGET_DIR}"

echo "==> Output:"
ls -lh "${OUT_DIR}"
