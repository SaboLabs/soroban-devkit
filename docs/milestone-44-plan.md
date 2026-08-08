# M44 — On-Chain ABI for Storage Decode

> Status: **Planned / Scheduled (post-M43, v2.5.0).** This is the next milestone
> after M43, assigned the ID M44 per the post-M43 roadmap audit recommendation (the
> C1 candidate: wire the M41 on-chain `ContractSpec` retrieval into the storage
> analyzer's ABI-aware decode path, mirroring M43). Planning only — no implementation
> is performed here. Title, objective, and scope are mirrored exactly in
> `ROADMAP.md §4`.

## 1. Current-State Confirmation
- `sdkt storage` has subcommands `Check`, `Estimate`, `Analyze` (enum `StorageAction`,
  main.rs:880). The `Storage` command carries `abi: Option<String>` (local WASM path)
  and `net: NetworkArgs`.
- In the `Storage` handler (main.rs:1345-1367) a single `contract_spec` value is
  computed ONCE, shared by every `StorageAction` arm: if `--abi <path>` is given, the
  local WASM is read and `sdkt_wasm::parse_contract_spec` produces a `ContractSpec`;
  otherwise `None`. That spec is then used for ABI-aware storage decoding (functions/
  events/types surfaced in Check/Analyze output).
- M41 already provides `inspect_contract` → `get_wasm_bytecode` → `parse_contract_spec`
  for a deployed contract, but `storage` can only use it via a manually-downloaded
  local WASM. `--abi-contract` does not exist on `storage` (it exists on `events` only,
  added by M43).
- Network/profile/mainnet-safety is already resolved via `resolve_rpc_client` (M39
  guard applies) in the `Storage` handler — same as M43.

## 2. Exact Gap Being Closed
`sdkt storage analyze --contract <id> --abi <local.wasm>` (and Check) can decode
storage ABI only from a LOCAL WASM file. To decode storage for a contract the user did
not build, they must first fetch its on-chain WASM manually. The M41 retrieval path is
not exposed to `storage`, even though M43 already proved the identical wiring for
`events`. This milestone closes that symmetric gap for storage.

## 3. Exact CLI / API Design
- Additive flag on the existing `Storage` command:
  `sdkt storage <action> --contract <id> --abi-contract <CONTRACT_ID> [net overrides]`
  (e.g. `sdkt storage analyze --contract <id> --abi-contract <id>`).
- `--abi <path>` unchanged; `--abi` and `--abi-contract` are mutually exclusive
  (controlled error "specify only one of --abi or --abi-contract").
- No `--abi-contract` → existing raw/non-ABI behavior unchanged.
- Validation: offline/RPC/contract-not-found/WASM/ABI failure → clean exit 1, no panic
  (mirrors M43).

## 4. Architecture and Existing Primitives Reused
- **M41 on-chain retrieval:** `inspect_contract(client, id)` → `get_wasm_bytecode(client,
  &wasm_hash)` → `parse_contract_spec(&bytes)`. No new RPC method, no new ABI parser.
- **Existing storage analyzer:** the computed `contract_spec: Option<ContractSpec>` is
  already threaded into every `StorageAction` arm; adding the on-chain source merely
  populates the same `Option` — zero change to the analyzer's decode logic. No new
  storage decoder.
- **M43 pattern:** the `events` handler already implements the exact
  mutual-exclusion + `inspect_contract`→`get_wasm_bytecode`→`parse_contract_spec` branch;
  this milestone copies that branch into the `Storage` handler's shared spec block.
- **Network/mainnet-safety:** inherited from `resolve_rpc_client` (unchanged).

## 5. Expected Files to Change
- `crates/sdkt-cli/src/main.rs` — add `abi_contract: Option<String>` to `Storage`;
  add mutual-exclusion check; add the on-chain ABI branch in the shared `contract_spec`
  block (reusing M41 retrieval). No change to the storage analyzer or any decoder.

## 6. Deliverables
- Production: `crates/sdkt-cli/src/main.rs` (Storage flag + on-chain ABI branch only).
- Tests: extend `crates/sdkt-cli/tests/` with a new `storage_abi_contract_test.rs`
  (hermetic): mutual-exclusion, offline graceful failure, existing local `--abi`
  regression, `--help` documents `--abi-contract`, and a deterministic fixture-based
  assertion that an on-chain `ContractSpec` reaches the storage analyzer's ABI output.
- Compatibility CI: extend `.github/workflows/compatibility.yml` with an "On-chain
  storage ABI decoding (M44)" step — committed deterministic fixture
  `tests/fixtures/onchain/storage-abi.json` (asserts actual decoded ABI fields:
  functions/events/types present), plus a network-guarded live attempt (non-fatal).
- Docs: `docs/cli.md` documents `--abi-contract` under `storage`; `ROADMAP.md` adds M44.

## 7. Non-Goals
- Remote plugin marketplace / hosted registry.
- Contract invocation / transaction submission / any write operation.
- Deployed-vs-deployed upgrade safety.
- New ABI parser / new storage decoder / new RPC method.
- New storage subcommand.
- Unrelated storage-system refactors.
- Version bump / tag / release / crates.io publish during implementation.
- Any later milestone (M45+).

## 8. Test Strategy
- **Hermetic:** unit/integration tests use a fixture `ContractSpec` (or the M41
  retrieval result substituted by a fixture) so the storage decode path is proven
  without network — asserting the decoded ABI fields (functions/events/types) the
  analyzer emits, not merely CLI startup.
- **Existing local `--abi` preserved:** regression test that `--abi <path>` still
  resolves and decodes (offline file-not-found → controlled error proves the branch alive).
- **Graceful failure:** offline / contract-not-found / WASM-unavailable / parse-error →
  clean exit 1, no panic.
- **No flaky CI:** live path network-guarded with committed-fixture fallback.

## 9. Compatibility CI Strategy
- Mirror M41/M42/M43: committed `storage-abi.json` is validated on EVERY run (asserts
  the ABI fields the storage analyzer would emit for a known spec); the live
  `sdkt storage analyze --contract <id> --abi-contract <id>` attempt runs only if RPC
  is reachable and never fails the workflow. CI never depends on live testnet.

## 10. Risks and Mitigations
- **Risk:** the shared `contract_spec` block lives above the `match action`; if a
  per-action arm shadows it, behavior could diverge. **Mitigation:** the block is
  already shared and unchanged by this milestone — only the `Option` source is added.
- **Risk:** mainnet guard bypass. **Mitigation:** `resolve_rpc_client` is untouched;
  the on-chain fetch uses the same `client`.
- **Risk:** regression in existing `--abi`. **Mitigation:** dedicated regression test.
- Overall risk: LOW — this is the proven M43 branch copied into an analogous handler.

## 11. Documentation / ROADMAP Changes
- `docs/cli.md`: add `--abi-contract <CONTRACT_ID>` under `storage` (ABI-aware storage
  decode from a deployed contract; mutually exclusive with `--abi`).
- `ROADMAP.md`: add M44 to §4 "Soroban Ecosystem Integration" as scheduled; correct the
  stale M43 "active/scheduled" wording to merged; keep the §6 backlog themes intact.
- `docs/milestone-44-plan.md` (this file).

## 12. Expected Release Impact
- Additive, non-breaking change (new optional flag on `storage`; existing commands
  unchanged). Ships in the next tag (e.g. 2.6.0) like M41/M42/M43. No version bump, tag,
  or release performed during planning.

## 13. Validation Performed During Planning
- Confirmed baseline: main HEAD ad1b405, M40/M41/M42/M43 merged, version 2.5.0, clean
  tree, no M44 in ROADMAP.
- Confirmed the `Storage` handler's `contract_spec` block (main.rs:1358-1367) is shared
  across all `StorageAction` variants, so one change covers Check/Estimate/Analyze.
- Confirmed `inspect_contract`/`get_wasm_bytecode`/`parse_contract_spec` (M41) and the
  M43 events branch are the exact primitives/patterns to reuse.
- This document is planning-only: no production `.rs` changed, no Cargo/version/tag
  change, no commit/push.

## Final Planning Validation
- Only planning/docs changes (`docs/milestone-44-plan.md` new; `ROADMAP.md` modified).
- No production `.rs` changed.
- No Cargo.toml / Cargo.lock / version change (stays 2.5.0).
- No tag / release / publish.
- No generated artifacts / `.sdkt-cache`.
- No implementation started.
- No later milestone (M45+) invented.
