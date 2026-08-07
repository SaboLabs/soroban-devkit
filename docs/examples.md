# Examples & Common Workflows

Every example below uses the real `sdkt` CLI. Commands that touch a live
network (a contract id, account, or RPC) are marked **(network)** and require
an RPC endpoint configured in `.sdkt.toml` or via the default testnet/public
RPC. Offline commands work anywhere.

## Offline (no network)

### Decode a base64 XDR `ScVal`

```bash
sdkt decode <BASE64> --type ScVal
sdkt decode <BASE64> --type ScVal --format json
sdkt decode --file payload.b64 --type TransactionEnvelope
```

### Offline WASM diff with upgrade-safety verdict

```bash
sdkt diff --old-wasm deployed.wasm --new-wasm candidate.wasm --upgrade-safety
sdkt diff --old-wasm deployed.wasm --new-wasm candidate.wasm --format json
```

### Static security audit of a contract

```bash
sdkt audit contracts/token/src/lib.rs
sdkt audit contracts/token/src/lib.rs --format json
sdkt audit contracts/token/src/lib.rs --disable MOVE-001
```

### WASM metadata + cache (offline cache inspection)

```bash
sdkt wasm cache info
sdkt wasm cache clear
```

## Network (requires RPC)

These need a configured network. Set it once:

```bash
sdkt init my-project --minimal   # scaffolds a project + .sdkt.toml
```

### Inspect a contract's ABI and storage

```bash
sdkt inspect <CONTRACT_ID> --abi contract.wasm
sdkt storage check <CONTRACT_ID> --abi contract.wasm
sdkt storage analyze <CONTRACT_ID>
sdkt storage estimate contract.wasm
```

### Transaction lifecycle

Build, validate, simulate, sign, and submit a Soroban transaction:

```bash
# 1. Create a local signing identity (offline)
sdkt identity generate alice

# 2. Build an unsigned envelope (offline)
sdkt tx build \
  --source <SOURCE_ACCOUNT> --sequence <SEQ> \
  --contract <CONTRACT_ID> --function hello \
  --output unsigned.xdr

# 3. Validate the envelope offline
sdkt tx validate --envelope unsigned.xdr

# 4. Simulate against the network to catch failures early (RPC)
sdkt tx simulate --envelope unsigned.xdr

# 5. Sign with the local identity (offline)
sdkt tx sign --input unsigned.xdr --output signed.xdr --identity alice --network testnet

# 6. Submit the signed envelope (RPC)
sdkt tx submit --envelope signed.xdr
```

`tx sign` is **fully offline** — it signs with a local ED25519 keystore
identity, so no RPC or secret exposure is involved. The `--network` flag
(`testnet` | `mainnet` | `futurenet` | `custom:<passphrase>`) only selects the
signature hash; signing never touches the network. `tx submit` / `tx simulate`
read the network from `.sdkt.toml` or default to testnet RPC.

### Events and account

```bash
sdkt events <CONTRACT_ID> --abi contract.wasm
sdkt account <ADDRESS>
sdkt fee estimate
```

### Identity / keystore

```bash
sdkt identity generate alice
sdkt identity list
sdkt identity show alice
sdkt identity default alice
sdkt identity delete alice
```

### Deploy

```bash
sdkt deploy --wasm contract.wasm --salt <SALT>
# Abort if the upgrade is not backwards-compatible:
sdkt deploy --wasm new.wasm --salt <SALT> --deny-breaking --old-wasm deployed.wasm
```

## CI gating (copy-paste)

Gate a PR on the static audit and a release on upgrade-safety. See
[ci-cd.md](ci-cd.md) for the full workflows.

### Audit on PR Workflow
Ensure privileged functions have authentication barriers:

```yaml
# .github/workflows/sdkt-audit.yml
on: [pull_request]
jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: naninu123/soroban-devkit/.github/actions/sdkt@main
        with:
          command: audit
          sdkt-version: v2.1.1
          target: contracts/token/src/lib.rs
          severity-threshold: critical
```

### Upgrade-Safety Workflow
Ensure the newly built `.wasm` is completely backward-compatible with what is currently on-chain:

```yaml
# .github/workflows/sdkt-upgrade-safety.yml
on:
  release:
    types: [published]
jobs:
  upgrade-safety:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: naninu123/soroban-devkit/.github/actions/sdkt@main
        with:
          command: upgrade-safety
          sdkt-version: v2.1.1
          old-wasm: builds/current.wasm
          new-wasm: builds/candidate.wasm
```
