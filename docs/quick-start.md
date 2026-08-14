# Quick Start — first-time user walkthrough

This guide takes a developer who has **never used `sdkt`** from zero to a
working inspection, audit, and upgrade-safety comparison in under five minutes.
Every command is copy-pasteable and runs **offline** (no RPC, no account, no
network) unless stated otherwise.

If you have not installed `sdkt` yet, follow
[README § Quick Start](https://github.com/SaboLabs/soroban-devkit#quick-start)
first. This document assumes `sdkt` is on your `PATH` and reports a version
when you run `sdkt --version`.

---

## Step 1 — Install

Choose **one** of the following.

**A. GitHub Release binary (no Rust toolchain required):**

1. Open the [Releases](https://github.com/SaboLabs/soroban-devkit/releases)
   page and download the asset for your platform:

   | Platform | Asset |
   |----------|-------|
   | Linux (x86_64) | `sdkt-x86_64-unknown-linux-gnu.tar.gz` |
   | macOS (Intel) | `sdkt-x86_64-apple-darwin.tar.gz` |
   | macOS (Apple Silicon) | `sdkt-aarch64-apple-darwin.tar.gz` |

2. Extract and run:

   ```bash
   tar -xzf sdkt-<your-platform>.tar.gz
   chmod +x sdkt
   ./sdkt --version
   # Optional: make it available system-wide
   sudo mv sdkt /usr/local/bin/
   ```

**B. Build from source (requires Rust 1.88.0+):**

```bash
git clone https://github.com/SaboLabs/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli
```

---

## Step 2 — Verify installation

```bash
sdkt --version
```

Expected output (version may be newer):

```text
sdkt 2.5.0
```

Then confirm the CLI is responsive:

```bash
sdkt --help
```

This lists every top-level command and subcommand. `sdkt` commands are
offline by default — only a handful (e.g. `inspect`, `storage`, `health`)
need an RPC endpoint, and they say so explicitly.

---

## Step 3 — Inspect a WASM

`sdkt` can read the public ABI and metadata of any compiled Soroban contract
WASM without a network. This repository ships two tiny fixtures you can use
right now.

```bash
sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_old.wasm
```

What this command does:

- Parses the WASM binary and reports its **size** and **SHA-256 hash**
  (useful for verifying a deployed contract matches your build).
- Lists **custom sections** (e.g. `contractspecv0`, `contractenvmetav0`,
  `contractmetav0`) that Soroban attaches to every contract.
- Shows **exported functions** and the **Contract Spec** — the public
  functions, custom types, and events the contract exposes.

Example output:

```
WASM Inspection Report: crates/sdkt-cli/tests/fixtures/us_old.wasm
========================================
Size: 198 bytes
SHA-256 Hash: 05befa136e7f0829a5051d97b032f355a5e65976397df90b224d141942dce46c
Version: 1

Custom Sections (1):
  - contractspecv0

Exported Functions (0):

Contract Spec Available: Yes
  Functions: 2
    - fn transfer(1) -> 0
    - fn mint(1) -> 0
  Custom Types: 1
  Events: 1
```

The two functions `transfer` and `mint` are the contract's public interface.
This is the same information a frontend or integrator would need to call it.

---

## Step 4 — Run an offline audit

`sdkt audit` performs static security analysis on contract **Rust source**,
catching common mistakes before deployment. Point it at any contract's
`src/lib.rs`. The example below audits a tiny throwaway contract written to a
temporary file — no repository fixture is required.

```bash
cat > /tmp/example_contract.rs <<'EOF'
#![no_std]
use soroban_sdk::{contract, contractimpl, Address};

#[contract]
pub struct Token;

#[contractimpl]
impl Token {
    pub fn transfer(_from: Address, _to: Address, _amount: u64) {
        // NOTE: intentionally missing require_auth() — sdkt audit will flag this
    }
}
EOF

sdkt audit /tmp/example_contract.rs
```

Interpreting the output:

- `Severity: 0 critical, 0 warning, 0 info (0 total)` with `No issues found.`
  means the analyzer found nothing to flag.
- `critical` findings (e.g. `AUTH-001/002/003` — missing auth checks) should
  block a deploy.
- `warning` findings (e.g. `MOVE-001` — a possible move-after-use of a local)
  are heuristic and worth a look but are not necessarily bugs.
- JSON mode (`--format json`) emits the same result as structured data for CI.

Audit runs entirely offline and is safe to gate every pull request on. See
[docs/examples.md](examples.md) for the audit-on-PR recipe.

---

## Step 5 — Compare two contracts

Before upgrading a deployed contract, confirm the new WASM is
**backwards-compatible** with what is already on-chain. `sdkt diff
--upgrade-safety` diffs two WASM files and renders a breaking-change verdict.

```bash
sdkt diff \
  --old-wasm crates/sdkt-cli/tests/fixtures/us_old.wasm \
  --new-wasm crates/sdkt-cli/tests/fixtures/us_new.wasm \
  --upgrade-safety
```

How to read the verdict:

- `Compatible: YES` — the new contract keeps every function, event, and type
  the old one exposed (plus any additions). Safe to upgrade.
- `Compatible: NO` — at least one **Breaking** change was detected. The
  `Breaking:` block lists exactly what changed:
  - **Removed function / event / type** — something callers depended on is
    gone.
  - **Changed signature** — a function's arguments or return type changed.
- The `Non-breaking:` block lists safe additions (new functions, events, or
  types) that do not break existing integrators.

Example output:

```
Upgrade Safety
==============

Compatible: NO

Breaking:
  - Changed signature: mint()
  - Removed event: Transfer
  - Removed type: Point

Non-breaking:
  - Added function: balance()
  - Added event: Mint
  - Added type: Circle
```

This tells you the upgrade changes `mint()`'s signature and drops the
`Transfer` event and `Point` type — a breaking change — while adding
`balance()`, a `Mint` event, and a `Circle` type. Use `--deny-breaking` with
`sdkt deploy` to abort an upgrade automatically if this verdict is `NO`.

---

## Step 5 — Sign a transaction (offline)

`sdkt` can sign a built transaction envelope with a local ED25519 identity,
**without any network or secret exposure**. First create an identity, then build
and sign an envelope.

```bash
# Create a local signing identity (stored in the keystore, never printed)
sdkt identity generate alice

# Build an unsigned envelope (offline)
sdkt tx build \
  --source <SOURCE_ACCOUNT> \
  --sequence <SEQ> \
  --contract <CONTRACT_ID> \
  --function hello \
  --output unsigned.xdr

# Validate it offline
sdkt tx validate --envelope unsigned.xdr

# Sign with the local identity (offline; --network selects the signature hash)
sdkt tx sign --input unsigned.xdr --output signed.xdr --identity alice --network testnet
```

The signed envelope in `signed.xdr` is ready to broadcast with `sdkt tx submit
--envelope signed.xdr` (requires RPC) or any compatible Stellar client. Signing
itself uses only the local keystore — no RPC call is made.

---

## Step 6 — Where to go next

You now know the three core offline workflows. Continue with:

- **[docs/examples.md](examples.md)** — copy-paste recipes for every subcommand
  (decode, storage, tx, deploy) and CI gating patterns.
- **[docs/compatibility.md](compatibility.md)** — which real-world Soroban
  contracts `sdkt` is validated against, and the compatibility matrix.
- **[docs/ci-cd.md](ci-cd.md)** — wire `sdkt audit` and `sdkt diff
  --upgrade-safety` into GitHub Actions to block bad PRs and unsafe releases.

For the full command reference, see [docs/cli.md](cli.md). For build/install
options and feature flags, see [docs/installation.md](installation.md).
