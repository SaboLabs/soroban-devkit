#!/usr/bin/env bash
# Network-gated smoke test for Soroban Public Testnet Walkthrough.
# Validates real read-only inspection against a stable public Testnet contract.
# CI remains green when network tests are skipped (SDKT_TESTNET_ENABLED unset or 0).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Network gate: Skip gracefully if not explicitly enabled
if [ "${SDKT_TESTNET_ENABLED:-0}" != "1" ]; then
  echo "INFO: SDKT_TESTNET_ENABLED is not set to 1. Skipping network-gated testnet smoke test."
  echo "PASS (SKIPPED)"
  exit 0
fi

# Locate sdkt binary across build targets or PATH
SDKT="${SDKT:-}"
if [ -z "$SDKT" ]; then
  for cand in \
    "${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/sdkt" \
    "${CARGO_TARGET_DIR:-$REPO_ROOT/target}/release/sdkt" \
    "$REPO_ROOT/target/debug/sdkt" \
    "$REPO_ROOT/target/release/sdkt" \
    "$(command -v sdkt || true)"; do
    if [ -n "$cand" ] && [ -x "$cand" ]; then
      SDKT="$cand"
      break
    fi
  done
fi

if [ -z "$SDKT" ] || [ ! -x "$SDKT" ]; then
  echo "SMOKE FAIL: sdkt binary not found. Build it with: cargo build -p sdkt-cli --bin sdkt"
  exit 1
fi

command -v jq >/dev/null 2>&1 || {
  echo "SMOKE FAIL: jq utility not found. Please install jq."
  exit 1
}

# Isolated temporary network directory to prevent mutating or leaving persistent profiles behind
WORK_DIR="$(mktemp -d)"
export SDKT_NETWORK_DIR="$WORK_DIR/networks"
mkdir -p "$SDKT_NETWORK_DIR"

cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

CONTRACT_ID="CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC"
PASSPHRASE="Test SDF Network ; September 2015"
RPC_URL="https://soroban-testnet.stellar.org"
PROFILE_NAME="testnet-smoke"

echo "== [1] Configure testnet network profile =="
"$SDKT" network add "$PROFILE_NAME" \
  --rpc-url "$RPC_URL" \
  --passphrase "$PASSPHRASE" \
  --description "Smoke Testnet Profile"

echo "== [2] Check network profile health =="
NET_STATUS=$("$SDKT" network check "$PROFILE_NAME" --format json)
echo "Network check output: $NET_STATUS"
REACHABLE=$(echo "$NET_STATUS" | jq -r '.reachable')
STATUS=$(echo "$NET_STATUS" | jq -r '.status')
if [ "$REACHABLE" != "true" ] || [ "$STATUS" != "healthy" ]; then
  echo "SMOKE FAIL: Network profile $PROFILE_NAME is not reachable or healthy (reachable=$REACHABLE, status=$STATUS)"
  exit 1
fi

echo "== [3] Inspect public contract metadata =="
INSPECT_OUT=$("$SDKT" inspect "$CONTRACT_ID" --network-profile "$PROFILE_NAME" --format json)
echo "Inspect output: $INSPECT_OUT"
INSPECT_CID=$(echo "$INSPECT_OUT" | jq -r '.contract_id')
WASM_HASH=$(echo "$INSPECT_OUT" | jq -r '.wasm_hash')
if [ "$INSPECT_CID" != "$CONTRACT_ID" ] || [ -z "$WASM_HASH" ] || [ "$WASM_HASH" = "null" ]; then
  echo "SMOKE FAIL: Inspect check failed (expected contract_id=$CONTRACT_ID, got contract_id=$INSPECT_CID, wasm_hash=$WASM_HASH)"
  exit 1
fi

echo "== [4] Check contract on-chain health =="
HEALTH_OUT=$("$SDKT" health --contract "$CONTRACT_ID" --network testnet --format json)
echo "Health check output: $HEALTH_OUT"
HEALTH_CID=$(echo "$HEALTH_OUT" | jq -r '.contract_id')
HEALTH_VERDICT=$(echo "$HEALTH_OUT" | jq -r '.health')
if [ "$HEALTH_CID" != "$CONTRACT_ID" ] || [ "$HEALTH_VERDICT" != "healthy" ]; then
  echo "SMOKE FAIL: Health check failed (expected contract_id=$CONTRACT_ID, health=healthy; got contract_id=$HEALTH_CID, health=$HEALTH_VERDICT)"
  exit 1
fi

echo "== [5] Query contract events =="
EVENTS_OUT=$("$SDKT" events "$CONTRACT_ID" --network-profile "$PROFILE_NAME" --format json)
echo "Events output: $EVENTS_OUT"
if ! echo "$EVENTS_OUT" | jq -e 'type == "array"' >/dev/null; then
  echo "SMOKE FAIL: Events check failed (expected JSON array)"
  exit 1
fi

echo "== [6] Storage analysis =="
STORAGE_OUT=$("$SDKT" storage analyze "$CONTRACT_ID" --network-profile "$PROFILE_NAME" --format json)
echo "Storage output: $STORAGE_OUT"
STORAGE_CID=$(echo "$STORAGE_OUT" | jq -r '.contract_id')
TOTAL_ENTRIES=$(echo "$STORAGE_OUT" | jq -r '.total_entries')
if [ "$STORAGE_CID" != "$CONTRACT_ID" ] || [ "$TOTAL_ENTRIES" = "null" ] || [ "$TOTAL_ENTRIES" -lt 1 ]; then
  echo "SMOKE FAIL: Storage analyze check failed (expected contract_id=$CONTRACT_ID with total_entries >= 1)"
  exit 1
fi

echo "PUBLIC TESTNET SMOKE PASS"
