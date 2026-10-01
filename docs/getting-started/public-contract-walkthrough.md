# Public Testnet Contract Walkthrough — Inspect, Health, Storage & Events

This guide provides a reproducible, zero-secret onboarding walkthrough for `sdkt` against a stable public Soroban Testnet contract. It demonstrates how to inspect contract metadata, verify network health, analyze on-chain storage, query contract events, and parse structured JSON outputs for CI workflows.

Unlike the deployment walkthrough which requires compiling WASM and funding accounts, this walkthrough runs immediately from a clean checkout without requiring keys, temporary test accounts, or private assets.

---

## Constants & Testnet Environment

The walkthrough targets the canonical Native Stellar Asset Contract (SAC) deployed on the public Stellar Testnet:

| Parameter | Value |
| :--- | :--- |
| **Contract ID** | `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC` |
| **Network Name** | `testnet` |
| **RPC Endpoint** | `https://soroban-testnet.stellar.org` |
| **Network Passphrase** | `Test SDF Network ; September 2015` |
| **Friendbot URL** | `https://friendbot.stellar.org` |

---

## Step 1 — Configuring the Network Profile

Create a named network profile for Testnet (or verify your existing one):

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
  "latest_ledger": 1045231,
  "protocol_version": 22,
  "configured_passphrase": "Test SDF Network ; September 2015",
  "endpoint_passphrase": "Test SDF Network ; September 2015",
  "friendbot_url": "https://friendbot.stellar.org",
  "error": null
}
```

---

## Step 2 — Inspecting the Contract Spec & Metadata

Query the on-chain contract instance to extract its code hash and exported ABI interface:

```bash
sdkt inspect CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --network-profile testnet
```

For automated CI ingestion, request structured JSON:

```bash
sdkt inspect CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --network-profile testnet \
  --format json
```

### Expected Output Schema:
```json
{
  "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
  "executable_type": "wasm",
  "wasm_hash": "6b3f68...",
  "spec_available": true,
  "functions": [
    {
      "name": "balance",
      "inputs": [{"name": "id", "type": "Address"}],
      "outputs": [{"type": "i128"}]
    },
    {
      "name": "transfer",
      "inputs": [
        {"name": "from", "type": "Address"},
        {"name": "to", "type": "Address"},
        {"name": "amount", "type": "i128"}
      ],
      "outputs": []
    }
  ]
}
```

---

## Step 3 — Verifying Contract On-Chain Health

Run the diagnostic health command to evaluate contract state reachability and storage integrity:

```bash
sdkt health --contract CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --network testnet
```

Structured JSON verification:

```bash
sdkt health --contract CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --network testnet \
  --format json
```

### Expected Output Schema:
```json
{
  "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
  "network": "testnet",
  "status": "healthy",
  "instance_live": true,
  "ttl_remaining_ledgers": 298412,
  "executable_reachable": true,
  "issues": []
}
```

---

## Step 4 — Querying Contract Events

Retrieve emitted contract events across recent ledger sequences:

```bash
sdkt events CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --network-profile testnet \
  --format json
```

### Expected Output Schema:
```json
{
  "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
  "events": [
    {
      "ledger": 1045210,
      "ledger_closed_at": "2026-10-01T18:22:10Z",
      "id": "0001045210-0000000001",
      "type": "contract",
      "topics": ["transfer", "G...", "G..."],
      "value": "100000000"
    }
  ]
}
```

---

## Step 5 — Storage Inspection & State Analysis

Inspect the contract's persistent, temporary, and instance storage footprints:

```bash
sdkt storage analyze CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --network-profile testnet \
  --format json
```

### Expected Output Schema:
```json
{
  "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
  "total_entries": 42,
  "categories": {
    "instance": 1,
    "persistent": 41,
    "temporary": 0
  },
  "ttl_status": {
    "min_ttl_ledgers": 128450,
    "max_ttl_ledgers": 518400,
    "expiring_soon_count": 0
  }
}
```

---

## CI Integration & Scripting Patterns

In continuous integration, validate contract health and status using standard tools like `jq`:

```bash
# Assert contract status is healthy
HEALTH=$(sdkt health --contract CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC --network testnet --format json)
STATUS=$(echo "$HEALTH" | jq -r '.status')
if [ "$STATUS" != "healthy" ]; then
  echo "Contract health check failed: $STATUS"
  exit 1
fi

# Assert instance is live with sufficient TTL
TTL=$(echo "$HEALTH" | jq '.ttl_remaining_ledgers')
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

When `SDKT_TESTNET_ENABLED` is omitted, the smoke script exits cleanly (exit code `0`), ensuring local builds and CI remain green in air-gapped or offline environments.
