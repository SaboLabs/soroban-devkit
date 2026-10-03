#!/usr/bin/env bash
# Offline onboarding smoke test for `sdkt`.
# Exercises real commands against committed repository fixtures/example sources.
# No network, no secrets. Exits non-zero on any unexpected output.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDKT="${SDKT:-$REPO_ROOT/target/debug/sdkt}"
EXAMPLES="$REPO_ROOT/examples"
FIX="$REPO_ROOT/crates/sdkt-cli/tests/fixtures"

fail() { echo "SMOKE FAIL: $1"; exit 1; }

command -v "$SDKT" >/dev/null 2>&1 || fail "sdkt binary not found at $SDKT (run: cargo build --bin sdkt)"

echo "== [1] version =="
# Single source of truth: the root workspace version (same idiom as the
# Release workflow's version-check). Keeping the expected version derived
# rather than hardcoded means a version bump cannot leave this check stale.
EXPECTED_VERSION="$(grep -m1 '^version = ' "$REPO_ROOT/Cargo.toml" | sed -E 's/version = "([^"]+)".*/\1/')"
[ -n "$EXPECTED_VERSION" ] || fail "could not read workspace version from $REPO_ROOT/Cargo.toml"
"$SDKT" --version | grep -q "$EXPECTED_VERSION" || fail "version != $EXPECTED_VERSION"

echo "== [2] wasm inspect (committed fixture) =="
OUT=$("$SDKT" wasm inspect "$FIX/us_old.wasm")
echo "$OUT" | grep -q "Contract Spec Available: Yes" || fail "wasm inspect: no contract spec"
echo "$OUT" | grep -q "fn transfer" || fail "wasm inspect: missing transfer fn"

echo "== [3] audit (committed example contract) =="
AUD=$("$SDKT" audit "$EXAMPLES/sample_token/src/lib.rs" --format json)
echo "$AUD" | grep -q "AUTH-001" || fail "audit: expected AUTH-001 finding missing"
echo "$AUD" | grep -q "admin_action" || fail "audit: expected admin_action in finding"

echo "== [4] decode (committed sample ScVal) =="
DEC=$("$SDKT" decode "$(cat "$EXAMPLES/sample_scval.b64")" --type ScVal)
echo "$DEC" | grep -q '"bool": false' || fail "decode: expected {\"bool\": false}"

echo "SMOKE PASS"
