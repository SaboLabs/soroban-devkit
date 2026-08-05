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

```bash
sdkt tx inspect <TX_HASH>
sdkt tx simulate <XDR>
sdkt tx submit <XDR>
sdkt tx build   # interactive typed envelope builder
```

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

```yaml
# .github/workflows/sdkt-audit.yml
on: [pull_request]
jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: naninu123/soroban-devkit/.github/actions/sdkt@main
        with:
          command: audit
          sdkt-version: v1.0.0
          target: contracts/token/src/lib.rs
          severity-threshold: critical
```
