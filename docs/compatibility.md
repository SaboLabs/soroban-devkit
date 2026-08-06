# Compatibility Matrix — Real-World Soroban Projects (M33)

Validated: 2026-08-06
sdkt version: **2.0.0** (`sdkt --version` → 2.0.0)
Toolchain: rustc 1.97.1, target `wasm32v1-none`

## Purpose

M33 validates that Soroban DevKit works against real, current open-source
Soroban contracts and establishes a baseline compatibility matrix to prevent
regressions. Projects are cloned read-only into a temporary directory; they are
**never modified**. Validation runs `sdkt` against their **compiled WASM
artifacts** and **Rust source files**.

## Tested Projects

All projects are sourced from the official Stellar `soroban-examples`
repository (https://github.com/stellar/soroban-examples, cloned `--depth 1`),
which contains canonical contract implementations maintained against the
current `soroban-sdk` (27.0.0 at validation time). They cover distinct contract
types:

| Project | Contract type | SDK dep | Artifact built |
|---------|--------------|---------|----------------|
| `token` | Fungible token (SIP-10 standard) | soroban-sdk 27.0.0 + soroban-token-sdk | `soroban_token_contract.wasm` |
| `atomic_swap` | Swap / escrow (utilities) | soroban-sdk 27.0.0 | `soroban_atomic_swap_contract.wasm` |
| `liquidity_pool` | DeFi AMM | soroban-sdk 27.0.0 + num-integer | `soroban_liquidity_pool_contract.wasm` |
| `timelock` | Claimable balance / timelock (utilities) | soroban-sdk 27.0.0 | `soroban_timelock_contract.wasm` |
| `single_offer` | DEX single-offer (DeFi) | soroban-sdk 27.0.0 | `soroban_single_offer_contract.wasm` |

Artifacts were compiled locally with:

```bash
cargo build --target wasm32v1-none --release
```

> Note: `soroban-sdk` ≥ 22 rejects the legacy `wasm32-unknown-unknown` target on
> rustc ≥ 1.82 (reference-types / multi-value unsupported). Use `wasm32v1-none`
> (Rust 1.84+) — which is what these artifacts used.

## Commands Executed & Results

### Offline commands (validated against real artifacts)

| Command | Token | Atomic Swap | Liquidity Pool | Timelock | Single Offer |
|---------|:-----:|:-----------:|:--------------:|:--------:|:------------:|
| `sdkt wasm inspect <file>` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `sdkt diff --old-wasm X --new-wasm X --upgrade-safety` (self) | ✅ | ✅ | ✅ | ✅ | ✅ |
| `sdkt audit <src.rs>` | ✅ (0 findings) | ✅ (5 MOVE-001) | ✅ (0 findings) | ✅ (2 MOVE-001) | ✅ (0 findings) |

`✅` = command executed successfully and returned a well-formed report.

Additional real-world spot checks:
- `sdkt diff --old-wasm token.wasm --new-wasm liquidity_pool.wasm --upgrade-safety`
  correctly reports **Compatible: NO** with 13 removed functions and 1 changed
  signature (`__constructor`) — confirming genuine breaking-change detection
  across distinct real contracts.
- `sdkt wasm inspect token.wasm` parses all three custom sections
  (`contractspecv0`, `contractenvmetav0`, `contractmetav0`), 15 exports, and the
  full 13-function ABI.
- JSON output (`-f json`) verified valid for `audit`, `diff`, and `inspect`.

### Online commands (documented, not run offline)

| Command | Status | Reason |
|---------|--------|-------|
| `sdkt inspect --abi <wasm> <contract_id>` | ⚠️ online | Requires a live on-chain contract ID + RPC endpoint |
| `sdkt health --contract <id> [--wasm]` | ⚠️ online | Requires a live on-chain contract ID + RPC endpoint |
| `sdkt storage --contract <id>` | ⚠️ online | Requires RPC + ledger access |
| `sdkt verify --contract <id> --wasm <file>` | ⚠️ online | Requires RPC to fetch on-chain WASM hash |

These are **not** failures — they are by-design online commands. They were
excluded from automated PASS/FAIL because the validation environment is
offline. Manual online runs against testnet/mainnet contract IDs are the
recommended follow-up (see Known Limitations).

## Compatibility Matrix Summary

| sdkt command | Offline | Real-world artifact tested | Result |
|--------------|:-------:|:--------------------------:|:------:|
| `wasm inspect` | yes | 5 compiled contracts | PASS |
| `diff --upgrade-safety` | yes | 5 contracts (self + cross) | PASS |
| `audit` | yes | 5 source trees | PASS |
| `inspect` (ABI-aware) | no | — | online-only |
| `health` | no | — | online-only |
| `storage` | no | — | online-only |
| `verify` | no | — | online-only |

## Known Limitations

1. **Online command coverage.** `inspect`, `health`, `storage`, and `verify`
   require a network RPC and a deployed contract. M33 validates only the
   offline surface. A future CI job should exercise these against a persistent
   testnet contract to close the gap.
2. **Build target coupling.** Current `soroban-sdk` (≥ 22) requires
   `wasm32v1-none` (not `wasm32-unknown-unknown`) on rustc ≥ 1.82. DevKit's
   `init` scaffold uses `soroban-sdk = "21.0.0"` (see M32) which still builds on
   the legacy target; contracts validated here used SDK 27 + `wasm32v1-none`.
3. **`audit` is heuristic, not a full borrow-checker.** MOVE-001 flags locals
   passed as call arguments multiple times (a possible move-after-use). On
   `atomic_swap` and `timelock` it surfaces 2–5 warnings that are false
   positives for `Env`/`Address` (copy types). They are non-blocking warnings,
   not errors.
4. **Example-set scope.** Validation used the official `soroban-examples` tree
   (token, swap, AMM, timelock, offer). Broader coverage (NFT mint contracts,
   account-abstraction, ZK verifiers) is recommended as more projects are added.

## How to Reproduce

```bash
# 1. Clone real contracts (read-only)
git clone --depth 1 https://github.com/stellar/soroban-examples /tmp/m33/examples

# 2. Build real WASM artifacts
cd /tmp/m33/examples
for p in token atomic_swap liquidity_pool timelock single_offer; do
  (cd $p && cargo build --target wasm32v1-none --release)
done

# 3. Run sdkt against them
sdkt wasm inspect /tmp/m33/examples/token/target/wasm32v1-none/release/*.wasm
sdkt diff --old-wasm <a>.wasm --new-wasm <b>.wasm --upgrade-safety
sdkt audit   /tmp/m33/examples/token/src/lib.rs
```
