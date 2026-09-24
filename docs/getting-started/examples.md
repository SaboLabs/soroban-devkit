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

# NOTE: `sdkt storage estimate` is NOT YET IMPLEMENTED (placeholder only —
# it prints a stub message and does not compute a real storage-cost estimate).

# Read a contract storage entry by its complete LedgerKey (base64 XDR)
sdkt storage read --contract <CONTRACT_ID> --key-xdr <BASE64_LEDGER_KEY>

# Extend TTL of the contract instance (and optional extra keys)
# --ledgers is relative: entries will live at least N ledgers past the current ledger.
# Example: 17280 ledgers ≈ 1 day at 5s/ledger.
sdkt storage extend --contract <CONTRACT_ID> --ledgers 17280 --identity my-deployer
```

### Transaction lifecycle

Build, validate, simulate, sign, and submit a Soroban transaction:

```bash
# 1. Create a local signing identity (offline)
sdkt identity generate alice

# 2. Build an unsigned envelope (offline; fee defaults to 100 stroops,
#    override with --fee <STROPS>)
sdkt tx build \
  --source <SOURCE_ACCOUNT> --sequence <SEQ> \
  --contract <CONTRACT_ID> --function hello \
  --output unsigned.xdr

# 3. Validate the envelope offline
sdkt tx validate --envelope unsigned.xdr

# 4. Simulate against the network to catch failures early (RPC)
sdkt tx simulate --envelope unsigned.xdr

# 4b. Simulate with ABI-aware result decoding (requires contractspecv0 WASM)
sdkt tx simulate --envelope unsigned.xdr --abi /path/to/contract.wasm

# 4c. Simulate with on-chain ABI from a deployed contract (no local WASM needed)
sdkt tx simulate --envelope unsigned.xdr --abi-contract C...CONTRACT_ID...

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

### Fund an identity (Testnet only)

Friendbot is a Testnet-only faucet — it does not exist on Mainnet. The network
profile you pass must carry an explicit `friendbot_url`. `sdkt identity fund`
removes the manual step of copying your public key and pasting it into a Friendbot
UI or making a raw `curl` call.

```bash
# Generate a local signing identity (offline)
sdkt identity generate my-deployer

# Fund it via Friendbot using a profile that has --friendbot set
sdkt identity fund my-deployer --network-profile testnet

# JSON output for scripting
sdkt identity fund my-deployer --network-profile testnet --format json
```

- Requires `--network-profile <NAME>` to be passed explicitly.
- The profile **must** have a friendbot URL (set via `sdkt network add --friendbot <url>`).
- If the profile has no Friendbot URL, the command fails with a clear error.
- HTTP 429 (rate limit) is reported explicitly — wait and retry.
- HTTP 5xx and malformed responses surface as errors.

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
# Auto-generate salt (simplest)
sdkt deploy --wasm contract.wasm

# Explicit salt for deterministic address
sdkt deploy --wasm contract.wasm --salt <40-HEX-CHARS>

# Abort if the upgrade is not backwards-compatible:
sdkt deploy --wasm new.wasm --salt <40-HEX-CHARS> --deny-breaking --old-wasm deployed.wasm
```

`--salt` is optional. When omitted, a random 20-byte salt is generated automatically.

### Read-only contract call

Invoke a contract function without signing or submitting a transaction.
Useful for querying state, checking return values, and debugging.

```bash
# Simple call (no args)
sdkt call C... balance

# With typed args
sdkt call C... transfer --args address:G...,u32:100

# JSON output for scripting
sdkt call C... balance --format json --network-profile testnet

# ABI-aware result decoding (requires contractspecv0 WASM)
sdkt call C... balance --abi /path/to/contract.wasm --format json --network-profile testnet

# Same, but the ABI comes from the deployed contract (no local WASM needed)
sdkt call C... balance --abi-contract C... --format json --network-profile testnet
```

- `--args` accepts typed values: `u32:N`, `u64:N`, `i32:N`, `i64:N`, `bool:true|false`, `string:text`, `address:G...`
- `--abi <wasm>` enables ABI-aware result decoding using the contract spec from a local WASM file. Without it, the raw base64 XDR result is shown.
- `--abi-contract <id>` resolves the ABI from the deployed contract over RPC instead (`inspect_contract` → `get_wasm_bytecode` → spec parse) — no local artifact needed. Mutually exclusive with `--abi`.
- When an ABI is supplied and the function is found in the spec, the decoded human-readable label appears in the output. JSON includes both raw and decoded fields.
- If the function is not in the ABI, the raw result is preserved and a warning is emitted to stderr.
- If `--abi` WASM is invalid/unparseable, the command fails with a clear error.
- No identity required — call is read-only via `simulateTransaction`
- No transaction is signed or submitted

### State-changing invoke

Run the full transaction lifecycle in one command: sequence fetch →
simulation → final envelope build → signing → submission → polling.

```bash
# Invoke with a local identity (signs + pays fees)
sdkt invoke C... increment --args u32:1 --identity alice --network-profile testnet

# JSON output for scripting
sdkt invoke C... set_admin --args address:G... --identity alice --format json --network-profile testnet
```

- `--args` uses the same `TYPE:VALUE` syntax as `call`, but is strict: an
  unknown type or a missing `TYPE:` prefix is an error (no base64 passthrough).
- `--identity` names a keystore identity (`sdkt identity generate`); the secret
  key never touches the command line.
- The fee is computed from the simulation (`minResourceFee` + 100 stroops
  inclusion fee); the footprint and auth entries come from the same simulation.
- Output shows the transaction hash, final status, fee, and result XDR.
  Exit code is non-zero when the transaction fails or is rejected.
- Relation to `tx build/sign/submit`: `invoke` is the one-command equivalent of
  `tx build` (with a real sequence + simulated fees) → `tx sign` →
  `tx submit --wait`. Use the `tx` subcommands when you need to inspect or
  modify the envelope between steps; use `invoke` for the common straight path.
- Limitations (core implementation): single-operation only, no ABI-aware
  result decoding of the return value, no `--fee` override, no dry-run flag.
  A live Testnet smoke test is documented here but NOT exercised in CI.

## CI gating (copy-paste)

Gate a PR on the static audit and a release on upgrade-safety. See
[ci-cd.md](../compatibility/ci-cd.md) for the full workflows.

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
