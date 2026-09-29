#!/usr/bin/env bash
# ============================================================================
# sdkt Testnet deployment smoke — manual-dispatch only.
#
# Triggered by .github/workflows/testnet-smoke.yml (workflow_dispatch). It is
# NOT wired into PR/push/release CI: it spends real Testnet resources and needs
# a Testnet secret.
#
# It reuses the existing deployment path end to end — `sdkt network add` ->
# `sdkt identity import/fund` -> `sdkt deploy` (upload WASM + create contract,
# each submit-and-poll to a terminal on-chain state) -> `sdkt invoke` ->
# `sdkt call`. No new deployment code, no mocking, no faked success: every
# artifact printed (contract ID, tx hashes) comes from a real Testnet response.
#
# Failure semantics (exit codes):
#   0  SUCCESS                 deploy + post-deploy verification passed
#   0  TESTNET NOT TESTED      required secret absent -> skips, NOT a failure
#   2  TESTNET GUARD FAILED    endpoint/passphrase is not Testnet -> refuse
#   3  INFRASTRUCTURE FAILURE  RPC/friendbot/runner unreachable after retries
#   4  DEPLOYMENT FAILURE      a transaction reached a terminal FAILED state
#   5  VERIFICATION FAILURE    deploy succeeded but read-back did not match
#
# Secret safety: STELLAR_TESTNET_SECRET is only ever passed on stdin, never on
# argv (so it does not appear in `ps`), never echoed, never written to a
# committed file, never included in an artifact or failure message. The whole
# isolated keystore is created with mktemp -d and removed on EXIT (trap).
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIX_DIR="$REPO_ROOT/tests/fixtures/testnet-smoke"
FIX_WASM="$FIX_DIR/target/wasm32v1-none/release/testnet_smoke_counter.wasm"

TESTNET_RPC="https://soroban-testnet.stellar.org"
TESTNET_PASSPHRASE="Test SDF Network ; September 2015"
TESTNET_FRIENDBOT="https://friendbot.stellar.org"

# Bounded, deterministic retry for TRANSIENT failures only.
TRANSIENT_RETRIES=3
TRANSIENT_BACKOFF=2   # seconds; attempt i sleeps i*TRANSIENT_BACKOFF

SDKT="${SDKT:-}"
if [ -z "$SDKT" ]; then
  for cand in "$REPO_ROOT/target/release/sdkt" "$REPO_ROOT/target/debug/sdkt" "$(command -v sdkt || true)"; do
    [ -n "$cand" ] && [ -x "$cand" ] && SDKT="$cand" && break
  done
fi

WORK_DIR="$(mktemp -d)"
IDENTITY_DIR="$WORK_DIR/identities"
NETWORK_DIR="$WORK_DIR/networks"
mkdir -p "$IDENTITY_DIR" "$NETWORK_DIR"
export SDKT_IDENTITY_DIR="$IDENTITY_DIR"
export SDKT_NETWORK_DIR="$NETWORK_DIR"

# Record the path (no secret, just a directory name) so CI can prove the trap
# actually removed the ephemeral keystore.
if [ -n "${SMOKE_WORKDIR_FILE:-}" ]; then
  printf '%s\n' "$WORK_DIR" > "$SMOKE_WORKDIR_FILE" || true
fi

# Never leave the ephemeral keystore behind, on any exit path.
cleanup() { rm -rf "$WORK_DIR"; }
trap cleanup EXIT

CONTRACT_ID=""
UPLOAD_TX=""
CREATE_TX=""
INVOKE_TX=""
COUNTER=""
DEPLOYER=""
STATUS="UNKNOWN"
emit_summary() {
  {
    echo "## Testnet deployment smoke"
    echo ""
    echo "- status: \`$STATUS\`"
    echo "- network: \`Testnet\`"
    echo "- rpc: \`$TESTNET_RPC\`"
    echo "- passphrase: \`$TESTNET_PASSPHRASE\`"
    echo "- contract_id: \`${CONTRACT_ID:-(none)}\`"
    echo "- upload_tx: \`${UPLOAD_TX:-(none)}\`"
    echo "- create_tx: \`${CREATE_TX:-(none)}\`"
    echo "- invoke_tx: \`${INVOKE_TX:-(none)}\`"
    echo "- counter_after_increment: \`${COUNTER:-(not read)}\`"
    echo "- deployer: \`${DEPLOYER:-(none)}\`"
  } >> "$GITHUB_STEP_SUMMARY"
}

finish() { # finish <STATUS> <exit-code>
  STATUS="$1"; local code="$2"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then emit_summary; fi
  echo "network: Testnet"
  echo "contract_id: ${CONTRACT_ID:-}"
  echo "upload_tx: ${UPLOAD_TX:-}"
  echo "create_tx: ${CREATE_TX:-}"
  echo "invoke_tx: ${INVOKE_TX:-}"
  echo "counter_after_increment: ${COUNTER:-}"
  echo "status: $STATUS"
  exit "$code"
}

# Classify an error blob: transient -> retryable, anything else -> fatal.
is_transient() {
  grep -qiE 'timed? out|timeout|connection reset|connection refused|could not connect|temporary|429|rate ?limit|50[023]|unavailable|network error' <<<"$1"
}

retry_transient() { # retry_transient <label> <cmd...>
  local label="$1"; shift
  local last=""
  for attempt in $(seq 1 "$TRANSIENT_RETRIES"); do
    if last="$("$@" 2>&1)"; then
      printf '%s\n' "$last"
      return 0
    fi
    if is_transient "$last"; then
      echo "  [$label] transient failure (attempt $attempt/$TRANSIENT_RETRIES); retrying" >&2
      echo "  [$label] detail: $(head -c 200 <<<"$last")" >&2
      sleep $((attempt * TRANSIENT_BACKOFF))
      continue
    fi
    # Non-transient: fail immediately, no retry.
    echo "  [$label] non-transient failure; not retrying" >&2
    echo "  [$label] detail: $(head -c 400 <<<"$last")" >&2
    return 22
  done
  echo "  [$label] still failing after $TRANSIENT_RETRIES attempts" >&2
  return 21
}

# ----------------------------------------------------------------------------
# 0. Required inputs
# ----------------------------------------------------------------------------
if [ -z "$SDKT" ]; then
  echo "sdkt binary not found (build it or set SDKT=)"; finish "INFRASTRUCTURE FAILURE" 3
fi
"$SDKT" --version
echo "sdkt: $SDKT"

SECRET="${STELLAR_TESTNET_SECRET:-}"
if [ -z "$SECRET" ]; then
  echo "STELLAR_TESTNET_SECRET is not set."
  echo "No deployment was attempted. This is a skip, not a failure."
  finish "TESTNET NOT TESTED" 0
fi

# ----------------------------------------------------------------------------
# 1. Deterministic fixture -> WASM (offline build, no network)
# ----------------------------------------------------------------------------
echo "== [1] build fixture WASM =="
(cd "$FIX_DIR" && cargo build --target wasm32v1-none --release) \
  || finish "INFRASTRUCTURE FAILURE" 3
[ -f "$FIX_WASM" ] || finish "INFRASTRUCTURE FAILURE" 3
echo "wasm: $FIX_WASM ($(stat -c%s "$FIX_WASM") bytes)"

# ----------------------------------------------------------------------------
# 2. Testnet guard — verify the RPC endpoint IS Testnet before any signing.
#    A profile's stored passphrase is not proof; ask the node for its own
#    passphrase via getNetwork. Refuse on any mismatch/ambiguity.
# ----------------------------------------------------------------------------
echo "== [2] network identity guard (getNetwork) =="
NET_JSON=""
if ! NET_JSON="$(retry_transient "getNetwork" curl -sS --max-time 30 -H 'Content-Type: application/json' \
    -X POST "$TESTNET_RPC" -d '{"jsonrpc":"2.0","id":"smoke-net","method":"getNetwork"}')"; then
  echo "RPC endpoint not reachable/verifiable"; finish "INFRASTRUCTURE FAILURE" 3
fi
ACTUAL_PASSPHRASE="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("result",{}).get("passphrase",""))' "$NET_JSON")"
if [ "$ACTUAL_PASSPHRASE" != "$TESTNET_PASSPHRASE" ]; then
  echo "endpoint passphrase: ${ACTUAL_PASSPHRASE:-<none>}"
  echo "expected:           $TESTNET_PASSPHRASE"
  echo "Refusing to deploy: endpoint is not the SDF Testnet."
  finish "TESTNET GUARD FAILED" 2
fi
echo "guard: endpoint reports Test SDF Network ; September 2015 -> OK"

# ----------------------------------------------------------------------------
# 3. Network profile (isolated store) + identity import (secret via stdin only)
# ----------------------------------------------------------------------------
echo "== [3] profile + identity =="
"$SDKT" network add testnet-smoke \
  --rpc-url "$TESTNET_RPC" \
  --passphrase "$TESTNET_PASSPHRASE" \
  --friendbot "$TESTNET_FRIENDBOT" >/dev/null \
  || finish "INFRASTRUCTURE FAILURE" 3

# Never put the secret in argv: `-` makes the CLI read stdin.
IMPORT_OUT="$(printf '%s' "$SECRET" | "$SDKT" identity import smoke - 2>&1)" \
  || { echo "identity import failed (non-transient): malformed secret?"; finish "TESTNET GUARD FAILED" 2; }
DEPLOYER="$(sed -n 's/^Public Key: //p' <<<"$IMPORT_OUT")"
case "$DEPLOYER" in
  G*) echo "deployer: $DEPLOYER";;
  *) echo "could not derive a G-address from the imported identity"; finish "TESTNET GUARD FAILED" 2;;
esac

# ----------------------------------------------------------------------------
# 4. Fund via Friendbot (Testnet-only mechanism; does not exist on Mainnet)
# ----------------------------------------------------------------------------
echo "== [4] fund via Friendbot =="
# A reused Testnet account is already funded; Friendbot answers 400
# "account already funded to starting balance". That is the desired end state,
# not a failure, so it is treated as success (idempotent).
FUND_RC=0
FUND_OUT="$(retry_transient "fund" "$SDKT" identity fund smoke --network-profile testnet-smoke 2>&1)" || FUND_RC=$?
if [ "$FUND_RC" -ne 0 ]; then
  if grep -qiE 'already funded' <<<"$FUND_OUT"; then
    echo "already funded: skipping Friendbot (account exists on Testnet)"
  elif [ "$FUND_RC" -eq 21 ]; then
    # retry_transient exhausted its bounded retries on a transient error.
    echo "friendbot still failing after $TRANSIENT_RETRIES attempts"; finish "INFRASTRUCTURE FAILURE" 3
  else
    # rc 22: deterministic failure (bad request, invalid account, …).
    echo "Friendbot funding failed (non-transient):"
    echo "$FUND_OUT" | head -c 400
    finish "DEPLOYMENT FAILURE" 4
  fi
else
  echo "funded: yes"
fi

# Confirm the account actually exists on Testnet (read back, not assumed).
if ! retry_transient "account" "$SDKT" account "$DEPLOYER" --network-profile testnet-smoke --format json >/dev/null 2>&1; then
  echo "account not readable on Testnet after funding"; finish "INFRASTRUCTURE FAILURE" 3
fi

# ----------------------------------------------------------------------------
# 5. Deploy (upload WASM + create contract) — real transactions, terminal state
# ----------------------------------------------------------------------------
echo "== [5] deploy =="
DEPLOY_JSON=""
if ! DEPLOY_JSON="$("$SDKT" deploy --wasm "$FIX_WASM" --identity smoke --network-profile testnet-smoke --format json 2>&1)"; then
  if is_transient "$DEPLOY_JSON"; then
    echo "deploy failed on a transient condition"; finish "INFRASTRUCTURE FAILURE" 3
  fi
  echo "deploy command failed:"; echo "$DEPLOY_JSON" | head -c 600
  finish "DEPLOYMENT FAILURE" 4
fi
read -r DEPLOY_STATUS CONTRACT_HEX UPLOAD_TX CREATE_TX <<<"$(python3 -c '
import json,sys
d=json.loads(sys.argv[1])
print(d.get("status",""), d.get("contractId",""), d.get("uploadHash",""), d.get("createHash",""))
' "$DEPLOY_JSON")"
echo "deploy status: $DEPLOY_STATUS"
echo "contract_hex:  $CONTRACT_HEX"
echo "upload_tx:     $UPLOAD_TX"
echo "create_tx:     $CREATE_TX"
if [ "$DEPLOY_STATUS" != "SUCCESS" ]; then
  echo "deployment did not reach a successful terminal on-chain state"
  finish "DEPLOYMENT FAILURE" 4
fi

# `sdkt deploy` reports the contract id as 32-byte hex. The rest of the CLI
# (`invoke` / `call` / `inspect`) takes the canonical `C…` StrKey form, so
# render it here with the standard strkey encoding (version byte 2<<3 + CRC16-XMODEM
# over the payload, base32). No extra dependency: this is the same encoding
# stellar-strkey implements.
CONTRACT_ID="$(python3 -c '
import sys, base64
def crc16(data):
    crc = 0
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc
h = sys.argv[1].strip()
if h.startswith("C"):
    print(h)          # already a strkey
    raise SystemExit
body = bytes([2 << 3]) + bytes.fromhex(h)
c = crc16(body)
print(base64.b32encode(body + bytes([c & 0xFF, c >> 8])).decode().rstrip("="))
' "$CONTRACT_HEX")"
echo "contract_id:   $CONTRACT_ID"
case "$CONTRACT_ID" in C*) : ;; *) echo "no valid contract ID returned"; finish "DEPLOYMENT FAILURE" 4 ;; esac

# Independent proof the contract exists on-chain (not just derived locally).
if ! retry_transient "inspect" "$SDKT" inspect "$CONTRACT_ID" --network-profile testnet-smoke >/dev/null 2>&1 \
   && ! retry_transient "inspect" "$SDKT" inspect "$CONTRACT_HEX" --network-profile testnet-smoke >/dev/null 2>&1; then
  echo "deployed contract is not addressable on-chain"; finish "VERIFICATION FAILURE" 5
fi

# ----------------------------------------------------------------------------
# 6. Post-deployment verification — write, then deterministic read
# ----------------------------------------------------------------------------
echo "== [6] invoke increment (write) =="
INVOKE_JSON=""
if ! INVOKE_JSON="$("$SDKT" invoke "$CONTRACT_ID" increment --identity smoke --network-profile testnet-smoke --format json 2>&1)"; then
  if is_transient "$INVOKE_JSON"; then
    echo "invoke failed on a transient condition"; finish "INFRASTRUCTURE FAILURE" 3
  fi
  echo "invoke command failed:"; echo "$INVOKE_JSON" | head -c 600
  finish "DEPLOYMENT FAILURE" 4
fi
read -r INVOKE_STATUS INVOKE_TX <<<"$(python3 -c '
import json,sys
d=json.loads(sys.argv[1])
print(d.get("status",""), d.get("hash",""))
' "$INVOKE_JSON")"
echo "invoke status: $INVOKE_STATUS"
echo "invoke_tx:     $INVOKE_TX"
[ "$INVOKE_STATUS" = "SUCCESS" ] || { echo "increment did not reach SUCCESS"; finish "DEPLOYMENT FAILURE" 4; }

echo "== [7] call get (read-only) and verify value =="
CALL_JSON=""
if ! CALL_JSON="$("$SDKT" call "$CONTRACT_ID" get --abi "$FIX_WASM" --network-profile testnet-smoke --format json 2>&1)"; then
  echo "read-back call failed:"; echo "$CALL_JSON" | head -c 600
  finish "VERIFICATION FAILURE" 5
fi
COUNTER="$(python3 -c '
import json,sys,re
d=json.loads(sys.argv[1])
v=str(d.get("result",""))
# With --abi the CLI prints the decoded label, e.g. "u32(1)"; without it the
# raw ScVal base64. Accept the numeric form.
m=re.search(r"\((\d+)\)$", v) or re.fullmatch(r"\d+", v)
print(m.group(1) if m else v)
' "$CALL_JSON")"
echo "counter_after_increment: $COUNTER"
if [ "$COUNTER" != "1" ]; then
  echo "expected counter == 1, got: $COUNTER"
  finish "VERIFICATION FAILURE" 5
fi

finish "SUCCESS" 0
