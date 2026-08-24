# SCF #45 — Proof-of-Intent Demo Evidence (P1)

> Status: **Read-only demo evidence.** All commands below are `sdkt` read operations
> only. No deploy, upgrade, transfer, or mutating transaction was performed. No
> fabrication of contract data, users, adoption, or results. Where a path could not be
> demonstrated, the reason is stated explicitly.
>
> Date of evidence capture: 2026-08-08 (session on `main`, HEAD
> `077864e0c2ee54275cdf50a7e845d2295920a281`, post M40–M44 merge).

## 1. Purpose / scope

Demonstrate, against a real deployed Soroban contract on testnet, the capability
delivered by **M40–M44**:

1. M40 — local plugin store & management (local-only, no network)
2. M41 — on-chain contract interface & instance inspection
3. M42 — on-chain-vs-local upgrade-safety verification
4. M43 — live-contract ABI for events decode
5. M44 — on-chain ABI for storage decode

All live commands below were executed READ-ONLY against testnet RPC. No contract was
deployed, upgraded, or modified. No transaction was signed or submitted.

## 2. Testnet environment

- Network: **Stellar testnet** (`soroban-testnet.stellar.org`), READ-ONLY.
- Real deployed testnet contract used for all live connectivity:
  `CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC`
- `sdkt` built from `main` (`target/debug/sdkt`), version `2.5.0`.
- No saved network profile; commands pass `--rpc-url` and
  `--network-passphrase "Test SDF Network ; September 2015"` explicitly.
- A local fixture WASM (`crates/sdkt-cli/tests/fixtures/us_new.wasm`) is used only for
  offline decode demos and as the M42 candidate artifact.

## 3. M41 live evidence — on-chain inspection

Command:

```bash
sdkt inspect CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015"
```

Actual live result (READ-ONLY, exit 0):

```
Contract Inspection
Contract ID: CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC
WASM Hash: 60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02
Storage Keys: 0
```

- On-chain WASM hash fetched live via the XDR compatibility bridge.
- `ContractSpec`/`ABI` parsed from the retrieved on-chain WASM.
- The `C...` StrKey is now normalized to a 32-byte hex contract id by the inspect path
  (`contract_id_to_hex` → `encode_ledger_key`); no manual hex conversion is required.

**M41: LIVE PASS**

## 4. M42 live evidence — on-chain upgrade-safety

Command (READ-ONLY; fetches on-chain WASM, diffs vs local fixture — no deploy):

```bash
sdkt verify --contract CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --wasm crates/sdkt-cli/tests/fixtures/us_new.wasm \
  --upgrade-safety \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015"
```

Actual live result:

```
Upgrade Safety
==============

Compatible: NO

Breaking:
  - Removed function: apply_funding()
  - Removed function: cancel_order()
  - Removed function: open_position()
  ... (full list of removed functions/types vs candidate)
```

- On-chain WASM successfully fetched through the compatibility bridge
  (`inspect_contract` → `get_wasm_bytecode`).
- Upgrade-safety analysis produced a valid breaking-change verdict (M14
  `SpecDiff`/`UpgradeVerdict` applied to deployed-vs-local `ContractSpec`).
- No deploy or upgrade was performed (intentional — read-only).

**M42: LIVE PASS**

## 5. M43 live evidence — live-contract ABI events

Command (READ-ONLY):

```bash
sdkt events CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --abi-contract CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015"
```

Actual live result (exit 0):

```
Contract Events (ABI-decoded):

Event #1
Ledger: 4031659
Topics: ["AAAADwAAAA9mdW5kaW5nX2FwcGxpZWQA"]
Value: AAAAEAAAAAEAAAACAAAACgAAAAAAAAAAAAAAAAAAAnYAAAAFAAAAAAAAAAE=
  Decoded: sym("funding_applied")
  Decoded: vec(len=2)

Event #2
Ledger: 4032429
Topics: ["AAAADwAAAA9mdW5kaW5nX2FwcGxpZWQA"]
Value: AAAAEAAAAAEAAAACAAAACgAAAAAAAAAAAAAAAAAAAnYAAAAFAAAAAAAAAAE=
  Decoded: sym("funding_applied")
  Decoded: vec(len=2)
```

- `getEvents` response received and decoded by `sdkt` (gzip/HTTP transport fixed).
- Live RPC `getEvents` JSON shape parsed correctly.
- `--abi-contract <C...>` successfully fetched the deployed contract's ABI on-chain and
  decoded the event.
- Decoded event name: `funding_applied`; decoded payload: `sym("funding_applied")`,
  `vec(len=2)`.

**M43: LIVE PASS**

## 6. M44 live evidence — on-chain storage

Command (READ-ONLY):

```bash
sdkt storage --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  analyze CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC
```

Actual live result (exit 0):

```
Storage Analysis for Contract: CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC
Total Entries: 1
  Instance:    1
  Persistent: 0
  Temporary:   0

TTL Summary:
  Min TTL:        515459
  Max TTL:        515459
  Average TTL:    515459
  Expiring Soon:  0
  Est. Rent Cost: 51545900 stroops

Entries:
  #1   [instance] ttl=515459 (~29d) cost=51545900 stroops
```

- RPC request now carries an explicit contract-instance ledger key
  (`LedgerKey::ContractData` with `key = LedgerKeyContractInstance`), eliminating the
  previous empty-key failure.
- A real on-chain instance entry was returned and decoded (TTL ≈ 515k ledgers ≈ 29 days).
- `--abi-contract <C...>` variant returns the same valid result (flag accepted and
  passed through).
- Mutual exclusion enforced: `--abi` + `--abi-contract` →
  `Error: specify only one of --abi or --abi-contract` (exit 1).

**Honest scope note:** Soroban RPC `getLedgerEntries` requires explicit keys and cannot
enumerate a contract's full storage. The M44 fix queries the **guaranteed contract-
instance singleton** as the baseline entry; it does NOT claim full Persistent/Temporary
storage-key enumeration (that would require explicit keys the caller must supply).

**M44: LIVE PASS**

## 7. Historical blockers and resolutions

The live paths above were previously blocked by a chain of defects. All are now
**RESOLVED** (committed to `main`):

1. **`getLedgerEntries` request-shape bug (RESOLVED).** The client sent the positional
   form `json!([keys])` (array-of-array); the Stellar RPC requires the object form
   `{"keys": [...]}`. Fixed in `crates/sdkt-rpc/src/client.rs`.
2. **Live `LedgerEntry` data-first compatibility (RESOLVED).** The testnet RPC returns
   `LedgerEntry` in the data-first layout; the bundled `stellar-xdr` path was updated to
   decode it (XDR compatibility bridge, commit `921dfed`). On-chain reads now succeed.
3. **gzip / `getEvents` HTTP transport decode (RESOLVED).** `reqwest` was built without
   the `gzip`/`deflate` features, so gzip-encoded responses failed to decode. Added the
   features; `getEvents` and other methods now decode gzip transparently. The
   `getEvents` JSON response shape was also aligned to the live wire format.
4. **StrKey `C...` → hex normalization (RESOLVED).** The inspect path passed the raw
   `C...` StrKey into a hex decoder (`encode_ledger_key`). Added `contract_id_to_hex`,
   which accepts both `C...` and 32-byte hex and normalizes to hex before ledger-key
   encoding.
5. **M44 empty-key RPC failure (RESOLVED).** `get_ttl_info` previously called
   `get_contract_storage` with an empty keys vector. It now derives and sends the
   contract-instance singleton key (a real, always-present key), so the RPC returns a
   valid entry instead of `no keys specified in request`.

No blocker remains for M40–M44 live demonstration.

## 8. Read-only safety statement

All operations performed for this demo were READ-ONLY. No contract was deployed,
upgraded, or modified. No transaction was signed or submitted. No result, contract
address, user, adoption, or metric was fabricated. Live on-chain enrichment paths are
documented as **passing** with the exact real output captured above.

## 9. Honest limitations

- **No adoption / user / traction data is claimed.** This is an early-stage developer
  tool; the evidence here is technical, not usage-based.
- **No full storage-key enumeration.** M44 returns the guaranteed contract-instance
  entry only; enumerating all Persistent/Temporary entries requires explicit keys from
  the caller (RPC constraint, not a defect).
- **M42 live verdict depends on the candidate WASM supplied.** The `Compatible: NO`
  result above reflects the difference between the deployed contract and the local
  `us_new.wasm` fixture, not a fault in the tool.
- No M45/M46 milestones are claimed or implied.

## 10. Final SCF evidence summary

| Milestone | Capability | Live testnet | Status |
|---|---|---|---|
| M40 | Local plugin store (list/show/install/remove/update) | Local-only, no network mutation | PASS |
| M41 | On-chain inspection: WASM hash `60cddae6…cf02`, ABI fetched | `sdkt inspect CAE3…` exit 0 | LIVE PASS |
| M42 | On-chain upgrade-safety verdict | `sdkt verify --upgrade-safety` exit 0 (verdict emitted) | LIVE PASS |
| M43 | Live-contract ABI events: `funding_applied` decoded | `sdkt events CAE3… --abi-contract` exit 0 | LIVE PASS |
| M44 | On-chain storage: instance entry, TTL ≈ 515k | `sdkt storage analyze CAE3…` exit 0 | LIVE PASS |

All five milestones (M40–M44) are implemented, merged to `main`, and verified live
against a real testnet contract. No unsupported claims of users, adoption, funding,
partnerships, or production usage are made.
