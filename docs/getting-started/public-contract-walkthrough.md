# Public Testnet Contract Walkthrough — Inspect, Health, Storage & Events

This guide provides a reproducible, zero-secret onboarding walkthrough for `sdkt` against a stable public Soroban Testnet contract. It demonstrates how to inspect contract metadata, verify network health, analyze on-chain storage, query contract events, and parse structured JSON outputs for CI workflows.

Unlike the deployment walkthrough which requires compiling WASM and funding accounts, this walkthrough runs immediately from a clean checkout without requiring keys, temporary test accounts, or private assets.

---

## Prerequisites

Before running the commands in this guide, ensure the following dependencies are installed and accessible on your system:

1. **Build `sdkt` from checkout**:
   ```bash
   cargo build -p sdkt-cli --bin sdkt
   ```
   Add the compiled binary to your `PATH`, or reference it directly via `target/debug/sdkt` (or `$CARGO_TARGET_DIR/debug/sdkt`).

2. **JSON query utility (`jq`)**:
   `jq` is required for parsing JSON CLI outputs in automated shell and CI scripts:
   ```bash
   # Ubuntu / Debian
   sudo apt-get install jq

   # macOS (Homebrew)
   brew install jq
   ```

---

## Constants & Testnet Environment

The walkthrough targets a stable, publicly deployed WASM contract on the public Stellar Testnet:

| Parameter | Value |
| :--- | :--- |
| **Contract ID** | `CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC` |
| **Network Name** | `testnet` |
| **RPC Endpoint** | `https://soroban-testnet.stellar.org` |
| **Network Passphrase** | `Test SDF Network ; September 2015` |
| **Friendbot URL** | `https://friendbot.stellar.org` |

---

## Step 1 — Configuring the Network Profile

Create a named network profile for Testnet:

```bash
sdkt network add testnet \
  --rpc-url https://soroban-testnet.stellar.org \
  --passphrase "Test SDF Network ; September 2015" \
  --friendbot https://friendbot.stellar.org \
  --description "Stellar Public Testnet"
```

Verify reachability, ledger sync, and protocol version:

```bash
sdkt network check testnet --format json
```

### Expected Output Schema:
```json
{
  "profile": "testnet",
  "rpc_url": "https://soroban-testnet.stellar.org",
  "reachable": true,
  "status": "healthy",
  "latest_ledger": 5069471,
  "protocol_version": 29,
  "configured_passphrase": "Test SDF Network ; September 2015",
  "endpoint_passphrase": null,
  "friendbot_url": null,
  "network_info_error": null,
  "error": null
}
```

---

## Step 2 — Inspecting the Contract Spec & Metadata

Query the on-chain contract instance to extract its WASM hash, code size, and exported ABI functions/types:

```bash
sdkt inspect CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --network-profile testnet
```

For automated CI ingestion, request structured JSON:

```bash
sdkt inspect CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --network-profile testnet \
  --format json
```

### Expected Output Schema:
```json
{
  "contract_id": "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
  "wasm_hash": "60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02",
  "wasm_size": 65152,
  "abi": {
    "functions": [
      "pause",
      "unpause",
      "upgrade",
      "get_order",
      "liquidate",
      "initialize"
    ],
    "events": [],
    "types": [
      "DataKey",
      "Order",
      "FeeTier",
      "PoolInfo",
      "Position"
    ]
  },
  "storage_summary": {
    "instance_entries": 0,
    "persistent_entries": 0,
    "temporary_entries": 0
  },
  "ttl_info": null,
  "storage_keys": []
}
```

---

## Step 3 — Verifying Contract On-Chain Health

Run the diagnostic health command to evaluate contract state reachability and storage integrity:

```bash
sdkt health --contract CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --network testnet
```

Structured JSON verification:

```bash
sdkt health --contract CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --network testnet \
  --format json
```

### Expected Output Schema:
```json
{
  "contract_id": "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
  "network": "testnet",
  "health": "healthy",
  "on_chain_wasm_hash": "60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02",
  "storage": {
    "total_entries": 1,
    "instance_entries": 1,
    "persistent_entries": 0,
    "temporary_entries": 0,
    "other_entries": 0,
    "ttl": {
      "minimum_ttl": 264353,
      "maximum_ttl": 264353,
      "average_ttl": 264353,
      "expiring_entries_count": 0,
      "estimated_rent_cost": 26435300
    }
  },
  "reasons": []
}
```

---

## Step 4 — Querying Contract Events

Retrieve emitted contract events across recent ledger sequences:

```bash
sdkt events CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --network-profile testnet \
  --format json
```

### Expected Output Schema:
Returns a JSON array of event items (or an empty array `[]` when no events fall within the current ledger window):

```json
[
  {
    "contract_id": "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
    "ledger": 5068458,
    "topics": [
      "AAAADwAAAAh0cmFuc2Zlcg=="
    ],
    "value": "AAAACgAAAAAAAAAAAAAAF0h26AA="
  }
]
```

---

## Step 5 — Storage Inspection & State Analysis

Inspect the contract's persistent, temporary, and instance storage footprints:

```bash
sdkt storage analyze CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --network-profile testnet \
  --format json
```

### Expected Output Schema:
```json
{
  "contract_id": "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
  "total_entries": 1,
  "instance_entries": 1,
  "persistent_entries": 0,
  "temporary_entries": 0,
  "other_entries": 0,
  "total_size_bytes": null,
  "ttl_summary": {
    "minimum_ttl": 264352,
    "maximum_ttl": 264352,
    "average_ttl": 264352,
    "expiring_entries_count": 0,
    "estimated_rent_cost": 26435200
  },
  "entries": [
    {
      "key": "AAAABgAAAAEJun0qJKNsneSH9Dq0zoes8HzyfDK+4rzzXiJybKPAbAAAABQAAAAB",
      "class": "instance",
      "current_ttl": 264352,
      "days_remaining": 15,
      "extension_cost_stroops": 26435200
    }
  ]
}
```

---

## CI Integration & Scripting Patterns

In continuous integration, validate contract health and status using standard tools like `jq`:

```bash
# Assert contract health verdict is healthy
HEALTH=$(sdkt health --contract CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC --network testnet --format json)
STATUS=$(echo "$HEALTH" | jq -r '.health')
if [ "$STATUS" != "healthy" ]; then
  echo "Contract health check failed: $STATUS"
  exit 1
fi

# Assert instance storage has sufficient TTL
TTL=$(echo "$HEALTH" | jq '.storage.ttl.minimum_ttl')
if [ "$TTL" -lt 50000 ]; then
  echo "Warning: Contract TTL low ($TTL ledgers remaining)"
fi
```

---

## Common RPC Errors & Remediation

| Error | Cause | Remediation |
| :--- | :--- | :--- |
| `HTTP 429 Too Many Requests` | Public RPC rate limiter triggered by rapid burst queries. | Add exponential backoff (e.g. 1-2s delay) or point `--rpc-url` to a dedicated RPC provider. |
| `Contract not found` / `-32600` | The contract ID does not exist on Testnet or was deployed on a different network (e.g. Futurenet). | Ensure the network passphrase is explicitly set to `Test SDF Network ; September 2015`. |
| `State archived` / `RestorePreamble required` | Storage entry expired past its TTL horizon. | Run `sdkt storage restore --contract <ID>` with appropriate footprint extension. |
| `RPC Timeout` / `Connection refused` | Public testnet RPC node undergoing scheduled maintenance. | Fallback to secondary testnet RPC endpoints or retry after brief delay. |

---

## Running the Automated Smoke Test

To validate this entire walkthrough end-to-end against live Testnet:

```bash
# Opt-in to live testnet execution
export SDKT_TESTNET_ENABLED=1

# Execute smoke test script
./scripts/testnet_public_contract_smoke.sh
```

When `SDKT_TESTNET_ENABLED` is omitted or set to `0`, the smoke script exits cleanly (exit code `0`), ensuring local builds and CI remain green in air-gapped or offline environments.
