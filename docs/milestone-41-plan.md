# M41 — On-Chain Contract Interface & Instance Inspection

> Status: **Scheduled (post-M40, v2.5.0).** Authorized from the roadmap "Broader
> Soroban ecosystem integration" backlog item, scoped down to a precise,
> testable objective.

## 1. Status

- Scheduled. No production code implemented yet. Planning/documentation only at
  this stage.
- Depends on existing (already-shipped) capabilities: `sdkt-rpc::inspect::inspect_contract`,
  `sdkt-rpc::wasm::get_wasm_metadata`, `sdkt-wasm::parse_contract_spec` /
  `parse_metadata`, `sdkt-storage::StorageAnalyzer`, `sdkt-xdr::extract_wasm_*`.

## 2. Motivation

`sdkt` can richly inspect a **local** WASM file (`sdkt wasm inspect` parses the
full `contractspecv0` ABI, exports, and metadata) but when given a **deployed
contract id** it returns almost nothing useful.

Concretely, `sdkt-rpc::inspect::inspect_contract` (the function behind
`sdkt wasm metadata --contract <id>`, `sdkt health`, and `sdkt verify`) only
populates `contract_id` and `wasm_hash`. The fields `wasm_size`,
`storage_summary`, `ttl_info`, and `storage_keys` are left `None`/empty, and
**the parsed on-chain ABI (functions/events/types) is never surfaced at all**.

Notably, `sdkt-rpc` *already* ships `get_wasm_metadata(client, wasm_hash)`, which
fetches the on-chain WASM bytecode by hash and runs `sdkt_wasm::parse_metadata`
on it — but `inspect_contract` never calls it. The capability exists; the
integration is missing. This is a wiring/integration gap, not a missing primitive.

Additionally, the Compatibility (Real-World Soroban) CI job only exercises
**offline** commands (`wasm inspect`, `diff --upgrade-safety`, `audit`). The
RPC-backed inspection path is never validated against a real deployed contract,
so any regression in on-chain data handling would go undetected.

## 3. Exact Objective

Enrich on-chain contract inspection so that `sdkt wasm metadata --contract <id>`
(and consequently `sdkt health` / `sdkt verify`) returns a **complete, useful
report**: the deployed contract's WASM size, full parsed ABI (functions, events,
custom types), storage summary, TTL info, and storage keys — by wiring the
*already-existing* `get_wasm_metadata` into `inspect_contract` and populating the
`ContractInspection` struct. Add an on-chain compatibility check against real
deployed reference contracts so the RPC path is regression-tested.

This is a single, testable milestone. It does **not** add new network protocols,
new RPC methods, or new CLI subcommands beyond what already exists.

## 4. User Problem

A developer working with an already-deployed contract (e.g. a token on testnet)
cannot ask `sdkt` "show me this contract's interface" from its id. Today:

```
$ sdkt wasm metadata --contract C...TOKEN_ID --network testnet
contract_id: C...
wasm_hash:    abcd...
# (wasm_size: None, storage: empty, ttl: None, functions: none shown)
```

The developer must already possess the WASM file to get any ABI — defeating the
purpose of on-chain inspection. After M41:

```
$ sdkt wasm metadata --contract C...TOKEN_ID --network testnet
contract_id: C...
wasm_hash:    abcd...
wasm_size:    24576
functions:    [mint, burn, transfer, balance, ...]   # from contractspecv0
events:       [transfer, mint, burn]
types:        [Asset, Balance]
storage:      instance=1 persistent=4 temporary=0
ttl:          min=1234 max=99999 expiring=0
```

## 5. Existing Evidence

- `crates/sdkt-rpc/src/inspect.rs`: `ContractInspection` struct has
  `wasm_size: Option<usize>`, `storage_summary: StorageSummary`,
  `ttl_info: Option<TtlInfoSummary>`, `storage_keys: Vec<StorageKeyInfo>` — all
  left `None`/empty in `inspect_contract`.
- `crates/sdkt-rpc/src/wasm.rs`: `get_wasm_metadata(client, wasm_hash)` already
  fetches bytecode by hash and calls `sdkt_wasm::parse_metadata` — **unused by
  `inspect_contract`**.
- `crates/sdkt-wasm/src/spec.rs`: `parse_contract_spec(raw_wasm)` returns a full
  `ContractSpec` (functions/events/types) — available but never invoked for
  on-chain WASM.
- `crates/sdkt-cli/src/main.rs`: `wasm metadata`, `health`, `verify` all call
  `inspect_contract` and therefore see the empty fields.
- `.github/workflows/compatibility.yml`: only offline commands validated; no
  `--contract`/network inspection step exists.

## 6. Deliverables

1. **Enrich `inspect_contract`** (`crates/sdkt-rpc/src/inspect.rs`):
   - After obtaining `wasm_hash`, call the existing `get_wasm_metadata(client, &wasm_hash)`
     to fetch + parse on-chain WASM.
   - Populate `wasm_size` from the returned `WasmMetadata`.
   - Parse the ABI via `sdkt_wasm::parse_contract_spec` on the fetched bytes and
     add a serializable `abi: ContractAbiSummary` field to `ContractInspection`
     (function names, event names, type names — derived from the existing
     `ContractSpec`, no new parsing logic).
   - Populate `storage_summary` / `ttl_info` / `storage_keys` by reusing the
     existing `StorageAnalyzer` result already computed in `contract_health`
     (move the shared storage aggregation into `inspect_contract` so all three
     callers benefit). Keep behavior read-only.
2. **Add `ContractAbiSummary` struct** (in `inspect.rs` or `sdkt-wasm`) — a thin
   serializable projection of `ContractSpec` (function/event/type name lists).
   No new ABI decoding; purely reusing `parse_contract_spec`.
3. **CLI output**: `wasm metadata --contract` (and `health`) naturally render the
   enriched fields via existing `format_pretty`/`format_json` paths. No new flags
   required; add a `--no-abi` switch only if needed to keep output compact
   (optional, non-goal unless trivial).
4. **Compatibility coverage**: extend `.github/workflows/compatibility.yml` with a
   **networked** step (guarded so it is skipped when no RPC/testnet access) that
   runs `sdkt wasm metadata --contract <known-testnet-contract> --network testnet`
   and asserts a non-empty ABI. Falls back to a recorded/cached fixture if the
   network is unavailable in CI (no CI flakiness from external RPC).

## 7. Non-Goals

- No new RPC methods, new JSON-RPC calls beyond the existing `getLedgerEntries`.
- No new top-level CLI subcommand; reuse `wasm metadata --contract`.
- No hosted registry / remote plugin marketplace (unchanged M40 scope boundary).
- No contract *calling* / invocation; inspection only.
- No ABI-diff or upgrade-safety for on-chain-vs-local (already exists offline via
  `sdkt diff`); M41 does not add on-chain-vs-on-chain diffing.
- No write operations; strictly read-only.
- No version bump, no tag, no release.
- No M42 / future milestone invented.

## 8. Architecture / Reuse Decisions

- Reuse `get_wasm_metadata` verbatim — do not duplicate bytecode fetch/parse.
- Reuse `parse_contract_spec` / `parse_metadata` (sdkt-wasm) — do not add a
  second ABI parser.
- Reuse `StorageAnalyzer` (sdkt-storage) for storage/TTL — do not re-implement.
- `ContractInspection` remains the single returned type; add fields only.
- `inspect_contract` stays the single chokepoint; `wasm metadata`, `health`,
  `verify` automatically inherit the enrichment with zero duplication.
- Network behavior unchanged (read-only `getLedgerEntries`); mainnet-safety
  guard from M39 still applies to mutating commands (not affected — M41 is read-only).

## 9. CLI/API Impact

- `sdkt wasm metadata --contract <id> [--network <n>]`: output gains `wasm_size`,
  `functions`, `events`, `types`, `storage`, `ttl`. No breaking change to JSON
  schema (additive fields).
- `sdkt health <id>`: storage/TTL already shown; ABI summary added when a
  `--wasm` is not supplied (it now fetches on-chain ABI).
- `sdkt verify <id> --wasm <local>`: unchanged behavior, but on-chain side is
  now richer.
- Public API: `ContractInspection` gains `abi: Option<ContractAbiSummary>` and
  `wasm_size` is now populated. Backward-compatible (`Option`).

## 10. Compatibility Strategy

- Extend `compatibility.yml` with a networked step that runs against a known
  testnet contract (e.g. an official Soroban token deployed on testnet) and
  asserts the ABI list is non-empty.
- Guard the step so CI does not fail when testnet RPC is unreachable: if the RPC
  call errors, fall back to a **cached JSON fixture** (committed under
  `tests/fixtures/onchain/`) that was captured from a real contract, and assert
  against that. This keeps the on-chain *code path* exercised in CI (when network
  allows) and regression-safe (when it does not).
- Offline compatibility (existing) remains unchanged.

## 11. Test Plan

`crates/sdkt-rpc/tests/inspect_enrich_test.rs` (or extend existing inspect tests):
- `inspect_contract` populates `wasm_size` and `abi` when `get_wasm_metadata`
  succeeds (mock/recorded response fixture for the `getLedgerEntries` call).
- `inspect_contract` returns `contract_id` + `wasm_hash` and `abi: None` gracefully
  when the WASM code entry is missing (ContractNotFound / code-not-found path).
- `ContractAbiSummary` projection matches `parse_contract_spec` output for a real
  WASM fixture (reuse an existing example WASM from compatibility fixtures).
- Storage/TTL fields populate when `StorageAnalyzer` returns data (recorded fixture).

`crates/sdkt-cli/tests/wasm_metadata_onchain_test.rs` (hermetic, network-gated):
- With `SDKT_NETWORK_DIR` set and a mock/fixture RPC, `sdkt wasm metadata
  --contract <id>` prints a non-empty `functions:` line.
- When network is unavailable, command still exits 0 with the partial report
  (graceful degradation, no panic).

## 12. CI / Validation Gates

- `cargo fmt --all --check`
- `cargo check --workspace`
- `cargo test --workspace` (incl. new inspect + cli on-chain tests)
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Compatibility (Real-World Soroban): existing offline checks + new on-chain
  step (network-guarded with fixture fallback).
- Docker `sdkt --help` smoke (no distribution change).

## 13. Documentation Changes

- `docs/cli.md`: document the enriched `wasm metadata --contract` output (ABI,
  size, storage, ttl).
- `docs/compatibility.md`: note the added on-chain inspection coverage + fixture
  fallback.
- `RELEASE_READINESS.md`: no release entry yet (M41 not released); only the
  "Remaining work" note may reference on-chain inspection as done once merged.
- `ROADMAP.md`: schedule M41 (see §4 / §6 update). No M42.

## 14. Release / Version Policy

- No version bump. M41 is a patch-level capability addition on the v2.5.0 line.
- No tag, no GitHub Release, no crates.io publish triggered by this milestone
  alone (publishing remains gated on a release cut, per repo convention).
- A release (e.g. v2.6.0) would be a separate, later decision.

## 15. Risks

- **Testnet RPC flakiness**: mitigated by fixture fallback in CI (no external
  dependency for green CI).
- **Large ABI output**: mitigated by summarizing to name lists, not full spec
  dump; optional `--no-abi` if needed.
- **Extra network round-trip**: `get_wasm_metadata` is one additional
  `getLedgerEntries` call; acceptable for an inspection command.
- **Regression in `health`/`verify`**: they inherit enrichment automatically;
  covered by existing + new tests.

## 16. Acceptance Criteria

- [ ] `inspect_contract` populates `wasm_size`, `abi` (functions/events/types),
      `storage_summary`, `ttl_info`, `storage_keys` from on-chain data.
- [ ] `sdkt wasm metadata --contract <id>` prints a non-empty `functions:` line
      for a real deployed contract (or the committed fixture when offline).
- [ ] `sdkt health <id>` and `sdkt verify <id>` inherit the enrichment with no
      code duplication.
- [ ] All validation gates green; compatibility job includes the on-chain step
      (network-guarded).
- [ ] No new RPC methods, no new CLI subcommand, no version bump, no tag.

## 17. Explicitly Deferred Work

- **Remote plugin marketplace layer** (hosted index, signing, `.sdktplugin`
  bundles) — remains unscheduled backlog from M40.
- **On-chain-vs-on-chain upgrade-safety diff** — offline `sdkt diff` already
  covers upgrade-safety; extending to two live contracts is a separate concern.
- **Contract invocation / read-call execution** — out of scope; inspection only.
- **Broader ecosystem integration** (deeper compatibility matrix beyond the
  on-chain inspection path) — tracked as future backlog, not part of M41.
