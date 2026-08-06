# Compatibility CI (M34)

Automated real-world compatibility validation for Soroban DevKit. This workflow
extends the manual M33 validation into GitHub Actions so that any regression in
`sdkt` against real Soroban contracts is caught on every PR and push to `main`.

## Purpose

- Prevent `sdkt` from silently breaking on **real, current** Soroban contracts.
- Run the same offline commands validated in M33 (`wasm inspect`,
  `diff --upgrade-safety`, `audit`) against compiled artifacts from the
  official `stellar/soroban-examples` repository.
- Fail fast: any `sdkt` command exiting non-zero (panic, parse failure, IO
  error) blocks the merge.

## Trigger

- `push` to `main`
- `pull_request` targeting `main`

## Projects tested

Cloned shallow + read-only from `https://github.com/stellar/soroban-examples`
into a temporary workspace (`${{ runner.temp }}/soroban-examples`). The upstream
repo is never modified.

| Contract | Type | Artifact |
|----------|------|----------|
| `token` | SIP-10 fungible token | `soroban_token_contract.wasm` |
| `atomic_swap` | Swap / escrow | `soroban_atomic_swap_contract.wasm` |
| `liquidity_pool` | DeFi AMM | `soroban_liquidity_pool_contract.wasm` |
| `timelock` | Claimable balance / timelock | `soroban_timelock_contract.wasm` |
| `single_offer` | DEX single-offer | `soroban_single_offer_contract.wasm` |

Build target: `wasm32v1-none` (required by `soroban-sdk` ≥ 22 on rustc ≥ 1.82;
`wasm32-unknown-unknown` is rejected by the Soroban environment).

## Commands executed

For each contract `c` (5 contracts):

```bash
sdkt wasm inspect <c>.wasm
sdkt diff --old-wasm <c>.wasm --new-wasm <c>.wasm --upgrade-safety
sdkt audit <c>/src/lib.rs
```

Plus one cross-contract sanity check (must report breaking changes, not error):

```bash
sdkt diff --old-wasm token.wasm --new-wasm liquidity_pool.wasm --upgrade-safety
```

All commands are offline. Online-only commands (`inspect --abi`, `health`,
`storage`, `verify`) are intentionally excluded — they require a live RPC and
deployed contract (see `docs/compatibility.md`).

## Notes on behavior

- `sdkt` returns exit `0` even when `audit` emits warnings or `diff` reports a
  breaking change. So `set -e` in the workflow only fails on genuine command
  failures (panics, parse errors, missing files). This is the intended
  regression signal.
- Cargo builds are cached via `Swatinem/rust-cache@v2` — once for the `sdkt`
  workspace and once for the cloned examples workspace — keeping runtime
  reasonable (subsequent runs are mostly the WASM compile + sdkt invocation).

## Estimated runtime

Cold (no cache): ~6–9 min
- sdkt debug build: ~2 min
- `soroban-examples` clone (shallow): ~30 s
- 5 contract WASM builds (`wasm32v1-none` release): ~2–3 min
- sdkt validation passes: < 30 s

Warm (cache hit): ~3–4 min (dominated by WASM compile + sdkt run).

## How to reproduce locally

```bash
# 1. Build sdkt
cargo build --bin sdkt
SDKT="$PWD/target/debug/sdkt"

# 2. Clone examples read-only
git clone --depth 1 https://github.com/stellar/soroban-examples /tmp/se
cd /tmp/se

# 3. Build a representative subset
for c in token atomic_swap liquidity_pool timelock single_offer; do
  (cd "$c" && cargo build --target wasm32v1-none --release)
done

# 4. Validate
cd /tmp/se
for c in token atomic_swap liquidity_pool timelock single_offer; do
  WASM=$(ls "$c/target/wasm32v1-none/release/"*.wasm | head -1)
  "$SDKT" wasm inspect "$WASM"
  "$SDKT" diff --old-wasm "$WASM" --new-wasm "$WASM" --upgrade-safety
  "$SDKT" audit "$c/src/lib.rs"
done
```

This mirrors exactly what `.github/workflows/compatibility.yml` runs on CI.
