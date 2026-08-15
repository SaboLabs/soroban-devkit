# Examples & Common Workflows

Every example below uses the real `sdkt` CLI. Commands that touch a live
network (a contract id, account, or RPC) are marked **(network)** and require
an RPC endpoint configured in `.sdkt.toml` or via the default testnet/public
RPC. Offline commands work anywhere.

## Committed example (recommended starting point)

The repository ships a self-contained, offline-runnable example so you can
reproduce the core workflow without creating `/tmp` files:

```
examples/
  sample_token/src/lib.rs   # minimal Soroban contract (intentionally has an AUTH-001 finding)
  sample_scval.b64          # a valid base64-encoded ScVal for `sdkt decode`
```

Build `sdkt` first, then run the deterministic smoke test (no network, no
secrets):

```bash
cargo build --bin sdkt
bash scripts/smoke_examples.sh
```

The script verifies, against the actual binary:

1. `sdkt --version` reports `2.5.0`.
2. `sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_old.wasm` shows a
   contract spec with `fn transfer`.
3. `sdkt audit examples/sample_token/src/lib.rs` reports `AUTH-001` on
   `admin_action` (the example's deliberate, unguarded privileged function).
4. `sdkt decode` on `examples/sample_scval.b64` returns `{"bool": false}`.

All four checks must pass for the smoke test to exit 0.

## Offline (no network)

### Decode a base64 XDR `ScVal`

A real, copy-paste example (offline — no network):

```bash
sdkt decode AAAAAAAAAAIAAAAAAAAABHRlc3Q= --type ScVal
# → { "bool": false }

sdkt decode AAAAAAAAAAIAAAAAAAAABHRlc3Q= --type ScVal --format json
sdkt decode --file payload.b64 --type TransactionEnvelope
```

The decoder also handles `TransactionEnvelope` and `ContractEvent` payloads the
same way.

### Offline WASM diff with upgrade-safety verdict

```bash
sdkt diff --old-wasm deployed.wasm --new-wasm candidate.wasm --upgrade-safety
sdkt diff --old-wasm deployed.wasm --new-wasm candidate.wasm --format json
```

### Static security audit of a contract

`sdkt audit` runs on contract **Rust source**. Write a tiny throwaway contract to
a temp file, then point the auditor at it — no project scaffold required:

```bash
cat > /tmp/token.rs <<'EOF'
use soroban_sdk::{contract, contractimpl, Address};

#[contract]
pub struct Token;

#[contractimpl]
impl Token {
    pub fn transfer(_from: Address, _to: Address) {}
    // NOTE: admin_action is privileged but missing require_auth() — audit flags it
    pub fn admin_action(_admin: Address) {}
}
EOF

sdkt audit /tmp/token.rs
sdkt audit /tmp/token.rs --format json
sdkt audit /tmp/token.rs --disable MOVE-001
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

### Network profiles

```bash
# Save a named network profile (referenced by other commands instead of full URLs)
sdkt network add testnet \
  --rpc-url https://soroban-testnet.stellar.org \
  --passphrase "Test SDF Network ; September 2015" \
  --friendbot https://friendbot.stellar.org \
  --description "Stellar testnet"

# List / inspect / remove
sdkt network list
sdkt network show testnet
sdkt network remove testnet

# Machine-readable output for scripting / CI
sdkt network show testnet --format json
```

### Using a profile with RPC commands

Once a profile exists, reference it from any RPC command instead of repeating
the full endpoint:

```bash
# Inspect / explore using the saved profile
sdkt inspect <CONTRACT_ID> --network-profile testnet
sdkt account <ADDRESS> --network-profile testnet
sdkt events <CONTRACT_ID> --network-profile testnet

# Override the profile inline when needed
sdkt inspect <CONTRACT_ID> --network-profile testnet \
  --rpc-url https://my-custom-rpc.example \
  --network-passphrase "Custom Network ; 2024"
```

Precedence (highest wins): explicit `--rpc-url` / `--network-passphrase` >
`--network-profile` > `.sdkt.toml` `[network]` > built-in testnet default.
Commands without these flags behave exactly as before.

### Deploy

```bash
sdkt deploy --wasm contract.wasm --salt <SALT>
# Abort if the upgrade is not backwards-compatible:
sdkt deploy --wasm new.wasm --salt <SALT> --deny-breaking --old-wasm deployed.wasm
```

## CI gating (copy-paste)

Gate a PR on the static audit and a release on upgrade-safety. See
[ci-cd.md](docs/ci-cd.md) for the full workflows.

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
      - uses: SaboLabs/soroban-devkit/.github/actions/sdkt@main
        with:
          command: audit
          sdkt-version: v2.5.0
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
      - uses: SaboLabs/soroban-devkit/.github/actions/sdkt@main
        with:
          command: upgrade-safety
          sdkt-version: v2.5.0
          old-wasm: builds/current.wasm
          new-wasm: builds/candidate.wasm
```
