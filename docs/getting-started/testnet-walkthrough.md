# Testnet walkthrough — identity → fund → deploy → invoke → events

This guide is the **on-chain companion** to the offline [Quick Start](quick-start.md). It takes a fresh `sdkt` install through the full Testnet loop the kit advertises — *from contract to chain, one flow*:

1. Create a signing identity
2. Add a Testnet network profile (with Friendbot)
3. Fund the identity
4. Scaffold + customize a tiny counter contract
5. Build the WASM
6. Deploy to Testnet
7. `invoke` (state-changing) and `call` (read-only)
8. Inspect events and storage

Every command below exists at HEAD and is copy-pasteable. Expected output blocks match the CLI's pretty-print format (placeholders like `<…>` stand in for hashes / addresses that differ per run).

***

## Prerequisites

| Requirement | Notes |
| --- | --- |
| `sdkt` on your `PATH` | See [installation](installation.md) or [Quick Start § Install](quick-start.md#step-1--install). Verify with `sdkt --version` (expects `2.5.0` or newer). |
| Rust toolchain **1.88.0+** | Needed to compile the example contract (`rustc --version`). |
| `wasm32-unknown-unknown` target | `rustup target add wasm32-unknown-unknown` |
| Stellar **Testnet** + Friendbot | Public RPC `https://soroban-testnet.stellar.org` and faucet `https://friendbot.stellar.org` must be reachable. Friendbot does **not** exist on Mainnet. |

> Offline inspection / audit / diff from the Quick Start are unchanged. This document only adds the on-chain path.

***

## Step 1 — Create a signing identity

```bash
sdkt identity generate alice
```

Expected output:

```
Identity 'alice' generated successfully.
Public Key: GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF
```

Your real `G…` public key will differ. Confirm it anytime with:

```bash
sdkt identity show alice
```

```
Identity: alice
Public Key: G…
```

The secret key stays in the local keystore (`SDKT_IDENTITY_DIR` if set; otherwise the default store). It is never printed.

***

## Step 2 — Add a Testnet network profile (with Friendbot)

`sdkt identity fund` requires a named profile that carries an explicit Friendbot URL. Create one once:

```bash
sdkt network add testnet \
  --rpc-url https://soroban-testnet.stellar.org \
  --passphrase "Test SDF Network ; September 2015" \
  --friendbot https://friendbot.stellar.org \
  --description "Stellar Testnet"
```

Expected output:

```
Network profile 'testnet' saved.
```

Inspect it:

```bash
sdkt network show testnet
```

```
Network profile: testnet
  RPC URL:         https://soroban-testnet.stellar.org
  Passphrase:      Test SDF Network ; September 2015
  Friendbot URL:   https://friendbot.stellar.org
  Description:     Stellar Testnet
```

(Field labels match `sdkt network show` at HEAD; if your build omits an empty description line, that is fine.)

***

## Step 3 — Fund the identity via Friendbot

```bash
sdkt identity fund alice --network-profile testnet
```

Expected output:

```
Identity Funded via Friendbot
  Identity:   alice
  Address:    G…
  Network:    testnet
  Endpoint:   https://friendbot.stellar.org
  Status:     SUCCESS
```

If the profile has no `--friendbot`, the CLI fails with a clear error telling you to re-run `sdkt network add --friendbot <url>`. HTTP 429 means Friendbot rate-limited you — wait and retry.

Optional: confirm the account is live on Testnet:

```bash
sdkt account $(sdkt identity show alice | awk '/Public Key/{print $3}') --network-profile testnet
```

***

## Step 4 — Scaffold a project and add a state-changing counter

```bash
sdkt init hello-counter
cd hello-counter
```

Expected output:

```
✓ Project 'hello-counter' created
  ✓ Cargo.toml
  ✓ src/lib.rs
  ✓ .sdkt.toml
  ✓ README.md
  ✓ .gitignore
  ✓ tests/basic.rs
✓ Ready to build — run: sdkt build
```

The default scaffold only exposes a read-only `hello`. Replace `src/lib.rs` with a tiny counter that **mutates instance storage** and **emits an event** so the later `invoke` / `events` / `storage` steps have something real to show:

```bash
cat > src/lib.rs <<'EOF'
#![no_std]
use soroban_sdk::{contract, contractimpl, symbol_short, Env};

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    /// Bump a counter stored in instance storage and emit an `inc` event.
    pub fn increment(env: Env) -> u32 {
        let key = symbol_short!("COUNT");
        let count: u32 = env.storage().instance().get(&key).unwrap_or(0) + 1;
        env.storage().instance().set(&key, &count);
        env.events().publish((symbol_short!("inc"),), count);
        count
    }

    /// Read the current counter (read-only).
    pub fn get(env: Env) -> u32 {
        let key = symbol_short!("COUNT");
        env.storage().instance().get(&key).unwrap_or(0)
    }
}
EOF
```

`sdkt build` requires a `[contracts.*]` table. The scaffold's `.sdkt.toml` only sets network/build defaults — append the contract entry:

```bash
cat >> .sdkt.toml <<'EOF'

[contracts.counter]
path = "."
EOF
```

***

## Step 5 — Build the WASM

```bash
sdkt build
```

Expected output (artifact path may vary slightly by Cargo package naming):

```
✓ Workspace built successfully
  ✓ counter -> ./target/wasm32-unknown-unknown/release/hello_counter.wasm
```

You may also see an advisory `✓ Wrote sdkt.lock` block — that is expected. Note the `.wasm` path printed after `->`; use it in the next step.

If you see `No [contracts] configured in .sdkt.toml`, re-check Step 4's append.

***

## Step 6 — Deploy to Testnet

```bash
sdkt deploy \
  --wasm target/wasm32-unknown-unknown/release/hello_counter.wasm \
  --identity alice \
  --network-profile testnet
```

Expected output (hashes / contract id are run-specific):

```
Deployment Result:
  WASM Hash: <64-hex>
  Contract ID: C…
  Upload Hash: <64-hex>
  Create Hash: <64-hex>
  Salt: <40-hex>
  Status: SUCCESS
```

Export the contract id for the remaining steps:

```bash
export CONTRACT_ID=C…   # paste from the deploy output
```

Salt is auto-generated when omitted. Pass `--salt <40-hex-chars>` for a deterministic contract id.

***

## Step 7 — Invoke (write) and call (read)

### State-changing invoke

`sdkt invoke` runs the full lifecycle in one command: fetch sequence → simulate → build → sign → submit → poll.

```bash
sdkt invoke "$CONTRACT_ID" increment \
  --identity alice \
  --network-profile testnet
```

Expected output:

```
Invocation Result:
  Contract: C…
  Function: increment
  Hash:     <64-hex>
  Status:   SUCCESS
  Fee:      <N> stroops
  Result XDR: <base64>
```

Exit code is non-zero if the transaction fails. Repeat the command — the counter advances on each successful invoke.

### Read-only call

`sdkt call` simulates only (no signing, no submission):

```bash
sdkt call "$CONTRACT_ID" get \
  --abi target/wasm32-unknown-unknown/release/hello_counter.wasm \
  --network-profile testnet
```

Expected output shape:

```
Contract:  C…
Function:  get
Result:    <decoded value or base64 ScVal>
Events:
```

With `--abi`, the result is decoded via the contract spec when possible. Without `--abi`, you still get the raw simulation result XDR.

***

## Step 8 — Events and storage

### Events

```bash
sdkt events "$CONTRACT_ID" \
  --abi target/wasm32-unknown-unknown/release/hello_counter.wasm \
  --network-profile testnet
```

Expected output shape (after at least one successful `increment`):

```
Contract Events (ABI-decoded):

Event #1
Ledger: <ledger-number>
Topics: […]
Value: …
```

If nothing has been emitted yet (or the RPC window has no matches), you will see:

```
Contract Events (ABI-decoded):
No events found.
```

Re-run Step 7's `invoke`, then query events again.

### Storage

```bash
sdkt storage analyze "$CONTRACT_ID" --network-profile testnet
```

Expected output shape:

```
Storage Analysis for Contract: C…
Total Entries: <N>
  Instance:    <N>
  Persistent: <N>
  Temporary:   <N>
```

A TTL summary block may follow when the analyzer has ledger TTL data. You can also run `sdkt storage check "$CONTRACT_ID" --network-profile testnet` for the TTL-oriented check report.

***

## What you just exercised

| Step | Command | Role |
| --- | --- | --- |
| Identity | `sdkt identity generate` / `show` | Local ED25519 keystore |
| Profile | `sdkt network add` / `show` | Named RPC + Friendbot |
| Fund | `sdkt identity fund` | Testnet faucet |
| Scaffold | `sdkt init` + counter source | Contract under test |
| Build | `sdkt build` | Optimized WASM artifact |
| Deploy | `sdkt deploy` | Upload + create on Testnet |
| Write | `sdkt invoke` | Signed state change |
| Read | `sdkt call` | Simulation-only query |
| Observe | `sdkt events` / `sdkt storage analyze` | Post-deploy inspection |

***

## Where to go next

* [Quick Start](quick-start.md) — offline inspect / audit / upgrade-safety diff (unchanged).
* [Examples & Common Workflows](examples.md) — per-command recipes, including CI gating.
* [CLI Reference](../reference/cli.md) — full flag lists for every subcommand.
* [Installation](installation.md) — crates.io, release binaries, feature flags.

Mainnet safety: `sdkt deploy` / `invoke` refuse implicit mainnet. To touch public network you must select it explicitly via `--network-profile` (or `--rpc-url` + `--network-passphrase`) whose passphrase is the public network passphrase.
