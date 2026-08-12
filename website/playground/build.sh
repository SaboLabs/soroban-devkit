#!/usr/bin/env bash
# Build the Web Playground's browser WASM runtime.
#
# The playground glue crate is intentionally excluded from the native workspace
# (see root Cargo.toml), so it is compiled explicitly for wasm32-unknown-unknown
# here. Output lands in website/playground/wasm/ and is served as a static
# asset alongside the playground page — no build step at deploy time.
#
# Requirements:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.127 --locked
#
# Usage: bash website/playground/build.sh
set -euo pipefail

cd "$(dirname "$0")/../.."   # repo root (website/playground -> website -> repo)

WASM_BINDGEN_VERSION="0.2.127"
OUT_DIR="website/playground/wasm"

echo "==> Building sdkt-playground for wasm32-unknown-unknown (release)"
cargo build -p sdkt-playground --release \
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
