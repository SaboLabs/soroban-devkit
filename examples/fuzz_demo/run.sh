#!/usr/bin/env bash
#
# run.sh — end-to-end demo of the `sdkt fuzz` workflow on examples/fuzz_demo.
#
# 1. Builds the contract WASM with the standard Soroban toolchain
#    (`cargo build --release --target wasm32v1-none`).
# 2. Runs one deterministic `sdkt fuzz campaign` against `compute`
#    (fixed seed 0, one case, `--expect-success compute`).
# 3. Asserts the expected `UNEXPECTED_ERROR` finding appears in the output;
#    exits non-zero otherwise (also on build/campaign errors).
#
# The built .wasm is not committed — this script builds it (target/ is
# gitignored). Run from anywhere: `bash examples/fuzz_demo/run.sh`.
#
# Prerequisites (the documented toolchain):
#   - Rust with the `wasm32v1-none` target (`rustup target add wasm32v1-none`)
#   - the `sdkt` binary on PATH (build: `cargo build --bin sdkt --release`
#     from the repo root; or set SDKT to the built binary's path)
set -uo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$script_dir"

SDKT="${SDKT:-sdkt}"
if ! command -v "$SDKT" >/dev/null 2>&1; then
    echo "ERROR: '$SDKT' not found on PATH — build it with" >&2
    echo "  cargo build --bin sdkt --release" >&2
    echo "  (from the repo root, or set SDKT to the binary path)" >&2
    exit 2
fi

wasm="target/wasm32v1-none/release/fuzz_demo.wasm"

# --- 1. Build the contract WASM -------------------------------------------
echo "==> Building $wasm"
if ! cargo build --release --target wasm32v1-none >/dev/null 2>&1; then
    echo "ERROR: cargo build failed" >&2
    echo "   Ensure Rust and the wasm32v1-none target are installed:" >&2
    echo "   rustup target add wasm32v1-none" >&2
    exit 2
fi

if [ ! -f "$wasm" ]; then
    echo "ERROR: expected WASM not found: $wasm" >&2
    exit 2
fi

# --- 2. Run one deterministic fuzz campaign -------------------------------
# `--expect-success compute` declares every call must succeed. The fuzzer's
# `zero` argument mutation drives `b` to zero, so `a / b` (the deliberate
# divide-before-multiply defect, MATH-001) traps the VM; a success was
# declared, so the trap is reported as an UNEXPECTED_ERROR finding.
echo "==> Running fuzz campaign (seed=0, cases=1, function=compute)"
artifact_dir="$(mktemp -d)"
trap 'rm -rf "$artifact_dir"' EXIT

set +e
output=$("$SDKT" fuzz campaign "$wasm" \
    --seed 0 \
    --cases 1 \
    --function compute \
    --auth no-auth \
    --expect-success compute \
    --format pretty \
    --artifact-dir "$artifact_dir" 2>&1)
status=$?
set -e

case "$status" in
    1)
        ;; # exit 1 = findings exist — this is the expected success path
    0)
        echo "ERROR: campaign completed with no findings (expected UNEXPECTED_ERROR)" >&2
        printf '%s\n' "$output" >&2
        exit 1
        ;;
    *)
        echo "ERROR: fuzz campaign failed with exit code $status" >&2
        printf '%s\n' "$output" >&2
        exit "$status"
        ;;
esac

# --- 3. Assert the expected finding appears -------------------------------
if printf '%s\n' "$output" | grep -q "UNEXPECTED_ERROR"; then
    printf '%s\n' "$output"
    echo "==> OK: expected UNEXPECTED_ERROR finding surfaced"
    exit 0
else
    echo "ERROR: expected UNEXPECTED_ERROR finding not found in campaign output" >&2
    printf '%s\n' "$output" >&2
    exit 1
fi
