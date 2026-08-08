# M43 — Live-Contract ABI for Events Decode

> Status: **Planned / Scheduled (post-M42, v2.5.0).** This is the next milestone
> after M42, assigned the ID M43 per the post-M42 roadmap audit (C1 recommendation:
> wire the M41 on-chain `ContractSpec` retrieval into the M10 event decoder). Planning
> only — no implementation is performed here. Title, objective, and scope are mirrored
> exactly in `ROADMAP.md §4`.

## 1. Current-State Audit

### What `sdkt events` currently accepts
The `Events` command (`crates/sdkt-cli/src/main.rs`) takes:
- `contract_id: String` (positional, required),
- `--format <json|pretty>`,
- `--abi <WASM_PATH>` (optional local WASM file used as the ABI source),
- `net: NetworkArgs` (network profile + `--rpc-url` / `--network-passphrase`
  overrides with M29 precedence; M39 mainnet-safety applied in `resolve_rpc_client`).

### How local WASM ABI is currently loaded
In the handler (lines ~2207-2216): when `--abi <path>` is set, the file is read and
`sdkt_wasm::parse_contract_spec(&wasm_bytes)` produces a `ContractSpec`. That spec is
then passed to `sdkt_xdr::abi_decode::decode_event_topics(&spec, &topic_scvals, &data_scvals)`
(M10) for each event. Without `--abi`, events are printed raw (base64 topics/value).

### Where event decoding currently happens
`decode_event_topics` in `crates/sdkt-xdr/src/abi_decode.rs` (M10) is the single
decode engine. It is invoked from exactly two sites in the `events` handler (JSON and
pretty paths). No other decoder exists.

### What M41 already provides for deployed contracts
- `sdkt-rpc::inspect::inspect_contract(client, contract_id)` → `ContractInspection`
  with `wasm_hash` (+ optional parsed ABI).
- `sdkt-rpc::wasm::get_wasm_bytecode(client, &wasm_hash)` → raw on-chain WASM bytes.
- `sdkt_wasm::parse_contract_spec(&bytes)` → `ContractSpec`.
This is the exact chain M42 already uses; it needs no new RPC method.

### Exact integration gap
`--abi` only accepts a LOCAL file path. To decode events for a contract the user did
not build, they must (a) manually fetch the on-chain WASM, (b) save it, (c) pass it to
`--abi`. The M41 retrieval path already solves (a)+(b) but is not exposed to `events`.

## 2. Milestone Objective
Allow `sdkt events` to decode a deployed contract's events using that contract's
on-chain ABI (fetched via the existing M41 path), without requiring a local WASM
artifact.

## 3. CLI / API Design

### Proposed syntax (additive flag)
```
sdkt events <CONTRACT_ID> --abi-contract <CONTRACT_ID> [--format json|pretty] [net overrides]
```
Note: `--abi-contract` reuses the same `<CONTRACT_ID>` the command already requires,
so it is natural to also accept a different id; the flag value is the source contract
whose on-chain WASM provides the ABI.

### Interaction with existing `--abi`
- `--abi <path>` (local WASM) — unchanged behavior.
- `--abi-contract <id>` (deployed) — NEW: fetch on-chain WASM via M41 path, parse to
  `ContractSpec`, use it for `decode_event_topics`.
- `--abi` and `--abi-contract` are mutually exclusive (controlled error if both given).

### Backward compatibility
- `sdkt events <id>` and `sdkt events <id> --abi <path>` behave exactly as today.
- The default (no ABI flag) still prints raw events.

### Validation
- `--abi-contract` with unreachable RPC / contract-not-found / WASM-not-fetched /
  ABI-parse-failure → clean `Error: …` message, exit 1, no panic (mirrors M42).
- `--abi` + `--abi-contract` together → controlled error ("specify only one of
  --abi / --abi-contract").

## 4. Architecture

- **Reuse M41 on-chain retrieval:** call `inspect_contract(client, id)` →
  `get_wasm_bytecode(client, &wasm_hash)` → `parse_contract_spec(&bytes)`. No new RPC
  method, no new ABI parser.
- **Reuse M10 decoding:** the resulting `ContractSpec` is passed to the existing
  `decode_event_topics` at the same two call sites. No duplicate decoding engine.
- **Reuse network/profile/mainnet-safety:** the handler already calls
  `resolve_rpc_client` (M39 guard applies). The new flag only changes how the spec is
  *obtained*, not how the network is reached.
- Add a small helper (e.g. `resolve_abi_spec`) returning `Option<ContractSpec>`
  from either `--abi` (local) or `--abi-contract` (on-chain) path, keeping the handler
  diff minimal.

## 5. Deliverables

### Production files expected to change
- `crates/sdkt-cli/src/main.rs` — add `--abi-contract` flag to `Events`; add the
  on-chain ABI resolution branch (reusing M41 retrieval); wire the resolved
  `ContractSpec` into the existing `decode_event_topics` calls. No change to
  `decode_event_topics`, `inspect_contract`, `get_wasm_bytecode`, or
  `parse_contract_spec`.

### Test files expected
- `crates/sdkt-cli/tests/events_integration_test.rs` (extend): hermetic tests for
  mutual-exclusion validation, offline graceful failure, and `--help` documents the
  flag.
- New `crates/sdkt-cli/tests/events_abi_contract_test.rs`: prove ABI-aware decoding
  from a *fixture* `ContractSpec` (deterministic, no network) by simulating the
  deployed-ABI path with a recorded `ContractSpec`/event fixture, asserting decoded
  labels/fields match the M10 engine output.

### Compatibility workflow changes
- Extend `.github/workflows/compatibility.yml` with an "On-chain events ABI decoding
  (M43)" step following the M41/M42 pattern: a committed fixture
  (`tests/fixtures/onchain/events-abi.json`) capturing a real decoded event (contract
  id, raw topics/value, and expected decoded labels/fields) is ALWAYS validated;
  a network-guarded live `sdkt events <id> --abi-contract <id>` attempt runs only if
  RPC is reachable and never fails the workflow.

### Documentation changes
- `docs/cli.md`: document `--abi-contract` under `events`.
- `docs/milestone-43-plan.md` (this file).
- `ROADMAP.md`: add M43 row (§4) and correct stale M42 "active/scheduled" wording.

## 6. Non-Goals
- Remote plugin marketplace / hosted registry.
- Contract invocation / transaction submission / any write operation.
- Deployed-vs-deployed upgrade safety (that is the C3 backlog item, out of scope).
- New ABI parser or new event-decoding engine (reuse M10).
- Unrelated event-system refactors.
- Version bump / tag / release during implementation.
- Any later milestone (M44/M45/…) — not invented here.

## 7. Testing Strategy
- **Hermetic:** unit/integration tests use fixture `ContractSpec` + event topics so
  the M43 path is proven without network (deterministic verdict parity with M10).
- **Existing local-WASM behavior preserved:** `events --abi <path>` tests remain
  green; add a regression assertion.
- **Deployed-ABI path testable from fixtures:** the on-chain retrieval result is
  substituted by a fixture `ContractSpec`; assert decoded labels/fields equal the M10
  engine output for the same input.
- **Graceful failure:** offline / contract-not-found / WASM-unavailable / parse-error
  → clean exit 1, no panic (assert stderr contains "Error", not "panic").
- **No flaky CI:** live path is network-guarded with committed-fixture fallback.

## 8. Compatibility CI Strategy
- Mirror M41/M42: committed fixture `tests/fixtures/onchain/events-abi.json` is
  validated on EVERY run (asserts actual decoded event labels/fields, not just CLI
  startup). Live `sdkt events <id> --abi-contract <id>` is attempted only if RPC
  reachable; failure is logged and non-fatal. CI never depends on live testnet.

## 9. Scope / Risk Assessment
- **Smaller/lower-risk than marketplace/registry:** reuses three existing primitives
  (`inspect_contract`, `get_wasm_bytecode`, `decode_event_topics`) with no new
  network surface, no new parser, no new engine — purely an additive CLI flag that
  swaps the ABI *source*.
- **Architectural risks:** none material; the only new logic is the
  mutual-exclusion check and the on-chain spec fetch, both already exercised by M41/M42
  and covered by their error-handling patterns. Risk of regression is low because the
  existing `--abi` path is untouched and tests guard it.

## 10. ROADMAP Update
- Add M43 to §4 "Soroban Ecosystem Integration" as scheduled.
- Correct stale M42 wording: M42 is merged (not "active/scheduled"); M43 is the next
  scheduled milestone.
- Keep all §6 backlog themes (remote marketplace, broader ecosystem, DX, hosted
  registry) intact. No subsequent milestone IDs invented.

## 11. Release Impact
- Additive, non-breaking change (new optional flag; existing commands unchanged).
- Ships in the next tag (e.g. 2.6.0) like M41/M42. No version bump, tag, or release
  performed during this planning phase.

## 12. Final Planning Validation
- Only planning/docs changes (`docs/milestone-43-plan.md` new; `ROADMAP.md` modified).
- No production `.rs` changed.
- No Cargo.toml / Cargo.lock / version change (stays 2.5.0).
- No tag / release / publish.
- No generated artifacts / `.sdkt-cache`.
- No implementation started.
- No later milestone invented.
