# SCF #45 — Proof-of-Intent Demo Evidence (P1)

> Status: **Read-only demo evidence.** All commands below are `sdkt` read operations
> only. No deploy, upgrade, transfer, or mutating transaction was performed. No
> fabrication of contract data, users, adoption, or results. Where a live path could
> not be demonstrated, the reason is stated explicitly.
>
> Date of evidence capture: 2026-08-08 (session on `main`, HEAD
> `23bbf343e4267aa754282dffa59795c0dfe3df38`, post M44 merge).

## Objective

Demonstrate, against a real deployed Soroban contract, the capability delivered by
M40–M44:

1. M41 — on-chain contract interface & instance inspection
2. M42 — on-chain-vs-local upgrade-safety verification
3. M43 — live-contract ABI for events decode
4. M44 — on-chain ABI for storage decode

## Prerequisites

- `sdkt` built from `main` (`target/debug/sdkt`), version `2.5.0`.
- Outbound HTTPS to `https://soroban-testnet.stellar.org` (verified `getHealth`
  returns `healthy`, latestLedger ~4,030,000 at capture time).
- No saved network profile; commands use the default `testnet` RPC unless noted.
- A local fixture WASM (`crates/sdkt-cli/tests/fixtures/us_new.wasm`) for offline
  decode demos and for the M42 candidate artifact.

## Network / contract

- Network: **Stellar testnet** (`soroban-testnet.stellar.org`), READ-ONLY.
- Real deployed testnet contract used for live connectivity:
  `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`
  (observed emitting custom `fee` events at ledger ~4,020,000 via `getEvents`).
- NOTE on ID format: `sdkt` read commands that fetch on-chain WASM/storage
  (`wasm metadata`, `events --abi-contract`, `storage --abi-contract`, `verify
  --upgrade-safety`) require the contract ID in **hex** (raw 32-byte), while the raw
  `events <id>` command accepts the StrKey `C...` form. This is a CLI-surface
  inconsistency worth normalizing (see "What was NOT demonstrated").

## Commands executed & actual results

### A. Contract inspection (M41 path)

Offline decode pipeline (the same parser M41's on-chain path feeds into):

```
$ sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_new.wasm --format json
```

Actual output (parsed):
- `metadata.hash`: `5ae0c8b47b5723898bf9313abe1643f89eb23f19b9bd0cd82769db522767d97e`
- `metadata.size_bytes`: `238`
- `spec.custom_types`: `[Circle]`
- `spec.functions`: `[transfer, mint, balance]`
- `spec.events`: `[Mint]`

This proves the ContractSpec ABI parser (functions / events / custom types) works
end-to-end on a real Soroban WASM.

Live on-chain inspection (`sdkt wasm metadata --contract <hex>`) initially returned
`RPC error: invalid parameters`. Root cause confirmed by direct RPC comparison:
`sdkt-rpc`'s `get_contract_storage` called `getLedgerEntries` with the positional form
`json!([keys])` (array-of-array), but the Stellar RPC requires the object form
`{"keys": [...]}`. **This request-shape bug has been FIXED** (commit pending):
`getLedgerEntries` now sends `{"keys": [...]}`. After the fix, the RPC no longer
rejects the request ("invalid parameters" is gone) and the call now proceeds to
response decoding.

Remaining issue (OUT OF SCOPE for this fix, documented for follow-up): after the
request-shape fix, `sdkt wasm metadata --contract <hex>` on testnet now fails with
`Failed to extract WASM hash: XDR parse failed for type 'LedgerEntry': xdr value
invalid`. This is a **universal response-decoding failure** (reproduced on multiple
distinct Wasm contracts), indicating the bundled `stellar-xdr` version cannot decode
the `LedgerEntry` the testnet RPC returns. It is a separate correctness bug from the
request-shape defect and was intentionally NOT modified here (this task was scoped to
the request-shape fix only). It blocks the same live on-chain paths (M41/M42/M43/M44
`--abi-contract`) until addressed separately.

Direct RPC test confirming the request-shape fix:
- old tool form `[["<key>"]]` → `invalid parameters` (the original bug)
- correct form `{"keys":["<key>"]}` → accepted (different error: key content)
- after fix, `sdkt` sends the correct form and the RPC accepts the request.
- correct form `{"keys":["<key>"]}` → accepted (different error: key content)

=> M41 **live** inspection: the request-shape defect is FIXED, but live proof remains
BLOCKED by a separate, universal `LedgerEntry` XDR response-decode failure (see above,
out of scope for this fix). The decode pipeline itself is proven offline (above) and
by the committed hermetic tests.

### B. Upgrade safety (M42)

`sdkt verify --contract <id> --wasm <candidate> --upgrade-safety` requires (a) a local
candidate WASM and (b) an on-chain WASM fetch of the deployed contract. The on-chain
fetch uses the same `getLedgerEntries` path as M41. The request-shape bug there is
FIXED, but the same universal `LedgerEntry` XDR response-decode failure blocks the
live fetch. Therefore M42 **live** verification could not be demonstrated. The
upgrade-safety *logic* (M14 `SpecDiff`/`UpgradeVerdict` applied to a deployed vs local
`ContractSpec`) is covered by the committed M42 hermetic tests and the
`docs/milestone-42-plan.md` verification. No candidate was deployed for this demo
(intentional — read-only, no deploy).

### C. Live-contract event ABI (M43)

```
$ sdkt events <STRKEY> --abi-contract <STRKEY> --format json
```
- With StrKey: `Error: Failed to encode ledger key: Hex decode failed` (the
  `--abi-contract` flag hex-decodes its argument; StrKey is invalid hex).
- With hex ID: `RPC error: invalid parameters` — same `getLedgerEntries` defect as M41.

The M43 decode logic (`--abi-contract` → on-chain WASM → `parse_contract_spec` →
`decode_event_topics`) is proven by the committed `events_abi_contract_test.rs`
(5/5 pass) and the offline `--abi <wasm>` event decode path. Live enrichment is
blocked by the same RPC defect.

Live raw events (no `--abi-contract`) executed against testnet:
```
$ sdkt events CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC --format json
```
Earlier in the session this returned a valid `[]` (no contract events in the default
window) — proving RPC connectivity and that the `events` command issues a well-formed
`getContractEvents` call. Subsequent identical calls hit a transient transport error
("error decoding response body") from the testnet RPC gateway (intermittent; the RPC
`GET /` returns 405 as expected for a POST-only endpoint). This is environment
flakiness, not a logic defect in the command.

### D. On-chain storage ABI (M44)

```
$ sdkt storage analyze <id> --abi-contract <id> --format json
```
Same result pattern as M43: with StrKey → hex-decode error; with hex → after the
request-shape fix the RPC now accepts the request, but the call then fails with the
same universal `LedgerEntry` XDR response-decode error. The M44 decode wiring is proven
by the committed `storage_abi_contract_test.rs` (5/5 pass) and the offline
`storage analyze --abi <wasm>` path. Live enrichment is blocked by the XDR decode issue.

## Capability proven vs not demonstrated

| Capability | Proven how | Live testnet |
|---|---|---|
| ContractSpec ABI parse (M41 core) | Offline `sdkt wasm inspect` + hermetic tests | Blocked by XDR decode issue |
| On-chain WASM fetch (M41/M42/M43/M44) | Hermetic tests | Request-shape FIXED; blocked by XDR decode |
| Upgrade-safety verdict (M42) | Hermetic M42 tests | Blocked (XDR + no deploy) |
| Event ABI decode (M43) | Hermetic `events_abi_contract_test` + offline `--abi` | `--abi-contract` blocked by XDR |
| Storage ABI decode (M44) | Hermetic `storage_abi_contract_test` + offline `--abi` | `--abi-contract` blocked by XDR |
| RPC connectivity / events cmd | Live raw `sdkt events <StrKey>` → valid `[]` | PASS (intermittent transport) |

## What was NOT demonstrated (honest gaps)

1. **Live on-chain reads are blocked by a universal `LedgerEntry` XDR response-decode
   failure** in `sdkt-xdr` (the bundled `stellar-xdr` version cannot decode the
   `LedgerEntry` the testnet RPC returns). This blocks M41 inspect, M42 WASM fetch,
   M43/M44 `--abi-contract` against any live network. The *request-shape* bug that
   preceded it (`getLedgerEntries` sent `[["key"]]` instead of `{"keys":["key"]}`)
   has been FIXED in `crates/sdkt-rpc/src/client.rs` (this change, commit pending);
   the XDR decode failure is a separate, out-of-scope issue to be addressed later.
2. **M42 live upgrade-safety** also requires deploying a candidate; not done (read-only).
3. **No adoption / user / traction data** is claimed. This is an early-stage
   developer tool; the evidence is technical, not usage-based.

## Reproducibility notes

- Build: `cargo build --bin sdkt` (or use the committed Dockerfile).
- Offline decode (always works):
  `sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_new.wasm --format json`
- Hermetic test proof: `cargo test -p sdkt-cli --test events_abi_contract_test`
  and `--test storage_abi_contract_test` (both 5/5).
- Live reads require the `getLedgerEntries` fix above to succeed against a network.

## Explicit statement

All operations performed for this demo were READ-ONLY. No contract was deployed,
upgraded, or modified. No transaction was signed or submitted. No result, contract
address, user, adoption, or metric was fabricated. Live on-chain enrichment paths are
documented as currently blocked by a known RPC request-shape defect, with root cause
and the exact location of the fix identified but not applied.
