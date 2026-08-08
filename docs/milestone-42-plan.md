# M42 — On-Chain Upgrade-Safety Verification

> Status: **Planned / Scheduled (post-M41, v2.5.0).** Authorized from the post-M41
> roadmap audit recommendation (the "On-chain upgrade-safety / compatibility
> verification" slice of the unscheduled "Broader Soroban ecosystem integration"
> backlog). This document is planning only — no implementation is performed here.
> Objective, title, and scope are mirrored exactly in `ROADMAP.md §4`.

## 1. Milestone Title
**M42 — On-Chain Upgrade-Safety Verification**

## 2. Status / Motivation
After M41, `sdkt` can fetch a deployed contract's on-chain WASM and parse its ABI
(`sdkt wasm metadata --contract <id>`). After M14, `sdkt diff --upgrade-safety
--old-wasm <A> --new-wasm <B>` already classifies breaking vs non-breaking changes
between two local WASM files using `SpecDiff` + `UpgradeVerdict`. What is missing is
the bridge: comparing a **live deployed contract** against a **candidate local WASM**
to answer "if I deploy this new WASM to replace the contract at <id>, will I break
callers?" — without manually downloading the on-chain WASM first.

## 3. Current Problem / Concrete User Gap
- `sdkt diff --upgrade-safety` only accepts two local file paths (`--old-wasm`,
  `--new-wasm`). A developer who wants to check deploy-safety against the contract
  actually live at `<contract-id>` must: (a) manually obtain the deployed WASM
  (no built-in command returns raw on-chain WASM bytes to a file), (b) save it,
  (c) run `diff --upgrade-safety` with that path. That is three steps and a manual
  download the tool already has primitives for.
- `sdkt verify --contract <id> --wasm <local>` (M22) confirms the on-chain WASM
  *hash* matches a local artifact, but does NOT report ABI/interface compatibility
  or an upgrade-safety verdict. Hash-equal is a subset of the question; most
  upgrade decisions are about *interface* changes, not byte equality.

## 4. Exact Objective
Enable a single command that fetches the on-chain WASM of a deployed Soroban
contract (via the existing M41 retrieval path), parses its `ContractSpec`, and runs
the existing M14 upgrade-safety engine (`SpecDiff` → `UpgradeVerdict`) against a
candidate local WASM — producing a breaking/non-breaking verdict. Reuse 100% of
existing primitives; add no new RPC method, no new parser, no new verdict engine.

## 5. Proposed User-Facing Workflow
Recommended (see §6 for rationale):
```
sdkt verify --contract <CONTRACT_ID> --wasm <candidate.wasm> --upgrade-safety [--network <name>] [net overrides]
```
- Baseline = the contract currently deployed at `<CONTRACT_ID>` (fetched on-chain).
- Candidate = the local `<candidate.wasm>` passed via `--wasm`.
- Output = the same `UpgradeVerdict` (`Upgrade Safety`, `Compatible: YES/NO`,
  added/removed/changed functions/events/types) already produced by M14, plus a
  short note that the baseline was the live deployed contract.

Offline-only equivalence is preserved: `sdkt diff --upgrade-safety --old-wasm <A>
--new-wasm <B>` keeps working unchanged for the pure local/local case.

## 6. Candidate CLI Surface — Decision
Options evaluated:
- **(A) Extend `sdkt diff --upgrade-safety`** with `--deployed <CONTRACT_ID>` that
  replaces `--old-wasm`. Problem: `diff` has no network/contract concept, no
  `NetworkArgs`, and no mainnet-safety hook. Adding network selection + the M39
  mainnet guard there duplicates machinery that `verify` already owns.
- **(B) Extend `sdkt verify` with `--upgrade-safety`.** ✅ CHOSEN. `Verify`
  already carries `--contract`, `--wasm` (local, `Option<String>`), `--network`,
  and `net: NetworkArgs` (profile + `--rpc-url`/`--network-passphrase` overrides
  with M29 precedence). The M39 mainnet-safety guard is applied inside
  `resolve_rpc_client`/`resolve_network_config`, so it is inherited automatically.
  `verify_contract` already fetches the on-chain contract and compares against a
  local WASM. Adding an upgrade-safety verdict is a natural extension of its
  existing "compare deployed vs local" responsibility.
- **(C) New subcommand.** Rejected: violates "one surface per milestone" principle
  and duplicates `verify`'s network/contract machinery.

**Recommendation: (B)** — extend `sdkt verify` with `--upgrade-safety`. No new
subcommand, no new RPC method, full reuse of M22/M29/M39/M41/M14 machinery.

## 7. Architecture / Reuse Decisions
- Reuse `sdkt-rpc::inspect::inspect_contract` to obtain `wasm_hash` + (already
  parsed) `abi` for the deployed contract. If only the hash is needed, also reuse
  `sdkt-rpc::wasm::get_wasm_bytecode(client, &wasm_hash)` → raw bytes →
  `sdkt_wasm::parse_contract_spec(&bytes)` to get the deployed `ContractSpec`.
  (M41 path — no new RPC method.)
- Reuse `sdkt_wasm::SpecDiff::diff_specs(deployed_spec, candidate_spec, …)` (or
  `diff_wasm` on the raw bytes) and `UpgradeVerdict::from_diff(&diff)` — the M14
  engine. NO parallel upgrade-safety engine.
- Reuse `verify_contract`'s existing client/network resolution and the M39
  mainnet-safety guard (no changes to that logic).
- The `UpgradeVerdict` struct + `print_upgrade_verdict` formatter (M14) are reused
  verbatim; only the source of the "old" side changes (deployed vs local file).
- No change to `AuditRule`, `RuleRegistry`, ABI parser, or `SpecDiff`/`UpgradeVerdict`
  internals.

## 8. Exact Modules / Files Expected to Change (implementation phase)
- `crates/sdkt-cli/src/main.rs` — add `upgrade_safety: bool` flag to `Verify`; in
  the `Verify` handler, when `--upgrade-safety` is set and `--wasm` is `Some`, fetch
  the deployed `ContractSpec` (via M41 path) and run `SpecDiff`/`UpgradeVerdict`
  against the local candidate, printing the verdict (reusing `print_upgrade_verdict`
  / JSON serialization).
- `crates/sdkt-cli/tests/` — new test file (hermetic + offline fallback) mirroring
  `upgrade_safety_integration_test.rs`.
- `.github/workflows/compatibility.yml` — add a network-guarded on-chain
  upgrade-safety step with a committed fixture fallback (mirrors M41's
  "On-chain inspection coverage" step).
- `tests/fixtures/onchain/` — add a committed `UpgradeVerdict` fixture (expected
  verdict for the `us_old`→`us_new` sample) so CI asserts the verdict, not just RPC.
- Reused (no change): `sdkt-rpc::inspect`, `sdkt-rpc::wasm`, `sdkt-wasm::spec_diff`,
  `sdkt-wasm::spec`, `verify_contract`.

## 9. Data Flow
```
user: sdkt verify --contract <ID> --wasm candidate.wasm --upgrade-safety --network testnet
  → resolve_rpc_client(net overrides / profile)        [M29/M39 mainnet guard applies]
  → inspect_contract(client, <ID>)                    [M41]  → wasm_hash (+ abi)
  → get_wasm_bytecode(client, &wasm_hash)             [M41]  → deployed raw bytes
  → parse_contract_spec(&deployed_bytes)              [sdkt-wasm] → deployed ContractSpec
  → fs::read(candidate.wasm) → parse_contract_spec    [sdkt-wasm] → candidate ContractSpec
  → SpecDiff::diff_specs(deployed, candidate, …)      [M14]
  → UpgradeVerdict::from_diff(&diff)                  [M14]
  → print_upgrade_verdict(&verdict) | serde_json      [M14 formatter]
```

## 10. Error / Network Behavior
- **RPC/network profile selection:** via existing `net: NetworkArgs` (flags >
  profile > `.sdkt.toml` > defaults) — unchanged from `verify` today.
- **Mainnet safety:** the M39 guard inside `resolve_rpc_client` refuses mainnet
  unless explicitly selected; `--upgrade-safety` inherits it with no extra code.
- **RPC failure / network unreachable:** command fails cleanly with an actionable
  "Error verifying contract: …" message (existing pattern) — no panic, exit code 1.
- **Contract not found:** `inspect_contract` returns `ContractNotFound` →
  surfaced as "Error verifying contract: contract not found" (matches M22).
- **Deployed WASM not retrievable** (code entry missing): `get_wasm_bytecode`
  errors → surfaced as a clean error; verdict not produced.
- **Candidate WASM invalid / unparseable:** `parse_contract_spec` fails →
  "Error: <path> is not valid WASM" (existing M22-style message).
- **Offline-only local comparison:** still available via
  `sdkt diff --upgrade-safety --old-wasm <A> --new-wasm <B>` — untouched.
- **Live-deployed vs offline distinction:** baseline source is implicit in the
  command — `--upgrade-safety` on `verify` means baseline = live contract; on
  `diff` it means baseline = local file. Output notes the baseline origin.

## 11. Security Considerations
- Read-only: no transaction submission, no contract invocation, no state change.
  Pure inspection + offline diff.
- No new network surface; reuses the single `sdkt-rpc` boundary.
- Mainnet-safety guard unchanged and fully inherited.
- No secrets, no key material touched; `verify` already uses the read-only keystore
  path only when signing is needed (not here).

## 12. Test Strategy
- **Unit/integration (hermetic):** reuse `tests/fixtures/us_old.wasm` and
  `us_new.wasm` (real Soroban WASMs). Simulate "deployed" by parsing `us_old.wasm`
  as the baseline `ContractSpec` and `us_new.wasm` as candidate — assert the same
  verdict the existing `upgrade_safety_integration_test.rs` already expects
  (`Compatible: NO`, `Changed signature: mint()`, `Added function: balance()`,
  `Added event: Mint`). This proves the engine path without network.
- **Offline graceful failure:** `sdkt verify --contract <unreachable> --wasm
  <valid.wasm> --upgrade-safety --network testnet` (no RPC) fails cleanly with an
  "Error" line and no panic.
- **CLI surface test:** `sdkt verify --help` documents `--upgrade-safety`.
- **No mock HTTP** (consistent with repo convention): live path is covered by the
  Compatibility CI step below, not by a unit test.

## 13. Compatibility CI Strategy (mirrors M41)
Add a step to `compatibility.yml`:
- A committed fixture `tests/fixtures/onchain/upgrade-verdict.json` captures the
  expected `UpgradeVerdict` for the `us_old`→`us_new` sample (the deterministic,
  offline-provable case). Asserted ALWAYS (no network).
- Attempt a live `sdkt verify --contract <known-testnet-id> --wasm <candidate>
  --upgrade-safety`; if RPC is unreachable, the step logs "live RPC unavailable"
  and passes (non-fatal). CI never depends on live testnet → not flaky.
- The step asserts the **verdict** (e.g. `Compatible: NO` / specific changed
  signature), not merely HTTP/RPC success.

## 14. Release Impact
- Ships as a normal feature in the next tag (e.g. 2.6.0). No breaking change to
  existing `diff`/`verify` behavior.
- **No version bump in this planning phase.** Workspace remains 2.5.0.

## 15. Explicit Non-Goals
- No new RPC method (reuse `inspect_contract` + `get_wasm_bytecode`).
- No new CLI subcommand.
- No new ABI parser or new upgrade-safety engine (reuse M14).
- No hosted registry, marketplace, remote plugin system.
- No contract invocation / write operations.
- No change to `AuditRule`, `RuleRegistry`, `SpecDiff`, `UpgradeVerdict` internals.
- No M43/M44 or any further milestone.
- No version bump, tag, publish, or release in this planning phase.

## 16. Acceptance Criteria
- `sdkt verify --contract <ID> --wasm <candidate.wasm> --upgrade-safety` returns the
  same `UpgradeVerdict` shape as `sdkt diff --upgrade-safety`, with baseline =
  the live deployed contract.
- Offline `diff --upgrade-safety` remains unchanged and passing.
- Hernetic tests + committed fixture prove the verdict deterministically.
- Compatibility CI step validates the verdict with a network-guarded, fixture-
  fallback design (never flaky).
- Mainnet-safety guard still applies to the new flag.
- All workspace quality gates green: `cargo fmt --all --check`, `cargo check
  --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`.

## 17. Rollback / Failure Considerations
- The change is additive (a new optional `--upgrade-safety` flag on `verify`); the
  existing `verify` (hash comparison) path is untouched. If the on-chain fetch
  fails, the command degrades to a clean error — no partial state.
- If the M14 engine is ever changed, this feature inherits the change automatically
  (single source of truth). No parallel logic to keep in sync.
- No migration, no schema change, no lockfile impact.
