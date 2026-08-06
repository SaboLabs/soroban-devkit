# M23 — Contract Health Report (Design Plan)

**Status:** Design only (no code changes)
**Target version:** v1.4.0 (post M22)
**Author:** IRONCLAW design pass
**Last updated:** 2026-08-05
**Depends on:** M21 (offline `wasm inspect`), M22 (`verify`), `sdkt-rpc::inspect_contract`, `sdkt-storage::StorageAnalyzer`, `sdkt-rpc::get_ttl_info`

---

## 1. Problem Statement

`soroban-devkit` has accumulated a rich set of **read-only per-contract** surfaces:

- `sdkt inspect <id>` / `sdkt-rpc::inspect_contract` — on-chain WASM hash + storage keys.
- `sdkt storage analyze <id>` / `sdkt-storage::StorageAnalyzer` — Instance/Persistent/Temporary classification + TTL summary + per-entry detail.
- `sdkt events <id>` — emitted events.
- `sdkt verify --contract <id> [--wasm <file>]` (M22) — on-chain hash vs local build hash.

However, these are **siloed**: answering the everyday question *"Is this contract healthy / what is its current posture?"* requires running 3–4 separate commands and mentally composing the results. Operators and CI pipelines want a **single, deterministic, machine-readable contract posture report** that aggregates the existing read-only data with a derived **health verdict** (Healthy / AtRisk / Critical).

M23 introduces `sdkt health --contract <ID>` — a read-only aggregator that calls the **existing** RPC/storage functions and produces one unified report. It invents **no new RPC method, no new crate, no new parsing logic**. It only orchestrates what already exists and adds a small, transparent verdict heuristic.

---

## 2. Goals

- G1. Provide a single `sdkt health --contract <ID>` command producing a unified contract posture report.
- G2. Reuse `sdkt-rpc::inspect_contract`, `sdkt-storage::StorageAnalyzer`, and `sdkt-rpc::get_ttl_info` exactly — no new network code.
- G3. Optionally fold M22 verification into the report when `--wasm <file>` is supplied (reuse `sdkt-wasm::parse_metadata` + the same comparison used by `verify_contract`).
- G4. Emit a derived `health` verdict: `Healthy` / `AtRisk` / `Critical`, with clear, rule-based reasons.
- G5. Support `--format json` and `--format pretty` (consistent with every other command).
- G6. Stay fully read-only (no simulate/submit/deploy).
- G7. Remain 100% backward compatible — additive command, no changes to existing commands/structs.

---

## 3. Non-goals

- NG1. **No new RPC endpoints.** Only `inspect_contract` + `get_ttl_info` (via `StorageAnalyzer`) are used.
- NG2. **No mutating operations.** No `simulate`, `submit`, `extend_ttl`, `restore`, or `deploy`.
- NG3. **No new crate.** All logic lives in `sdkt-cli` glue (same pattern as M22's `verify_contract`).
- NG4. **No new storage/instance classification logic.** Reuse `StorageAnalyzer` output verbatim.
- NG5. **No historical/time-series TTL tracking.** Only the current snapshot.
- NG6. **No mainnet-specific or SCF-grant tooling** (explicitly deferred to the Post-1.0 backlog).
- NG7. **No automatic remediation suggestions beyond the explicit verdict reasons.**

---

## 4. User Stories

- US1. As a contract operator, I run `sdkt health --contract C...` and get one screen telling me the WASM hash, storage breakdown, TTL risk, and an overall verdict — without piping 4 commands.
- US2. As a CI pipeline, I run `sdkt health --contract C... --format json` and gate on `health == "critical"` (or non-zero exit) to fail a deploy check.
- US3. As a developer, I run `sdkt health --contract C... --wasm ./target/wasm32v1/contract.wasm` and see whether the deployed bytecode matches my local build *inside* the posture report.
- US4. As an auditor, I feed `--format json` into a dashboard to track posture across many contracts.
- US5. As a user on a flaky network, if the contract is missing I get a clear `ContractNotFound` error and exit 1, not a panic.

---

## 5. CLI UX

New top-level command, sibling to `verify` / `inspect`:

```
sdkt health --contract <CONTRACT_ID> [--wasm <WASM>] [--network <NET>] [--format <pretty|json>]
```

Argument behavior (mirrors M22 `verify`):

| Flag | Required | Default | Meaning |
|------|----------|---------|---------|
| `--contract` (`-c`) | yes | — | Stellar contract ID (`C...`) |
| `--wasm` | no | — | Optional local WASM to verify against the on-chain hash |
| `--network` (`-n`) | no | `testnet` | Network label for the report (RPC endpoint still resolved from `.sdkt.toml`, same as M22 — see §16) |
| `--format` (`-f`) | no | `pretty` | `pretty` or `json` |

Exit codes (consistent with M22 and the rest of the CLI):

| Code | Meaning |
|------|---------|
| `0` | Report produced (regardless of `health` verdict — `AtRisk`/`Critical` are *data*, not process failures) |
| `1` | Operational failure: contract not found, RPC error, bad/missing `--wasm`, bad format, network error |

> Note: A `Critical` health verdict still exits `0` (the command succeeded; the *verdict* is critical). CI gating should inspect the `health` field / `verification_status`, not the exit code. This matches the M22 contract: a `Mismatch` also exits `0`.

---

## 6. Command Examples

```bash
# Full posture, human-readable
sdkt health --contract CABCDEFGHIJKLMNOP

# Machine-readable for CI / dashboards
sdkt health --contract CABCDEFGHIJKLMNOP --format json

# Posture + verify deployed bytecode against local build
sdkt health \
  --contract CABCDEFGHIJKLMNOP \
  --wasm ./target/wasm32v1/contract.wasm

# Explicit network label
sdkt health --contract CABCDEFGHIJKLMNOP --network testnet
```

Expected pretty output (healthy):
```
Contract Health Report
=======================
Contract ID : CABCDEFGHIJKLMNOP
Network     : testnet
Health      : HEALTHY

On-chain WASM : 3b9f...c2 (verified against local: YES)
Storage:
  Total Entries: 12
    Instance:    1
    Persistent:  9
    Temporary:   2
TTL:
  Min TTL:        518400
  Max TTL:        518400
  Average TTL:    518400
  Expiring Soon:  0
  Est. Rent Cost: 240000 stroops

Verdict: Contract posture is healthy. WASM verified, no entries expiring soon.
```

Expected pretty output (at risk):
```
Contract Health Report
=======================
Contract ID : CABCDEFGHIJKLMNOP
Network     : testnet
Health      : AT RISK

On-chain WASM : 3b9f...c2 (verified against local: NO — MISMATCH)
Storage:
  Total Entries: 3
    Instance:    1
    Persistent:  1
    Temporary:   1
TTL:
  Min TTL:        1200
  Max TTL:        518400
  Average TTL:    173200
  Expiring Soon:  2
  Est. Rent Cost: 40000 stroops

Verdict: 2 storage entries expiring soon (< 30 days). On-chain WASM does NOT
match the supplied local file (rebuild & redeploy or confirm correct artifact).
```

---

## 7. Architecture

Layering (identical discipline to M22):

```
sdkt-cli  (new `Commands::Health` arm + `ContractHealthReport` struct
          + `contract_health()` orchestrator + `derive_verdict()` helper)
   │  calls (read-only)
   ├─ sdkt-rpc::inspect_contract(client, id)      → wasm_hash, storage_keys
   ├─ sdkt-storage::StorageAnalyzer
   │     .inspect_contract_storage(id)            → StorageReport (classification + TTL)
   └─ sdkt-wasm::parse_metadata(bytes)           → only if --wasm supplied (offline)
```

- **No new crate.** `contract_health()` is a `sdkt-cli` glue function (exactly the M22 pattern).
- **No new RPC.** `inspect_contract` returns `ContractInspection { wasm_hash, storage_keys, … }`; the `StorageAnalyzer` internally calls `sdkt-rpc::get_ttl_info`. Both already exist.
- **Reuse M22 compare logic.** The local-vs-onchain comparison reuses the *same* `sdkt-wasm::parse_metadata` + equality check already proven in M22 (`verification_outcome` lives in `sdkt-cli` and can be called directly — no duplication). If M22's `verification_outcome` is not in scope to import, replicate the 3-line comparison inline (it is 3 lines; acceptable and avoids cross-test coupling). Preferred: call the existing `verification_outcome(on_chain, local)` helper.
- **Verdict derivation** (`derive_verdict`) is a new, pure, testable function — the only genuinely new logic in M23.

---

## 8. Data Flow

```
CLI parses `--contract/--wasm/--network/--format`
        │
        ├─ (offline, first) if --wasm: fs::read + sdkt_wasm::parse_metadata
        │       → (local_hash, local_size)  | error → "Error reading WASM" / "not valid WASM", exit 1
        │
        ├─ build client = SorobanRpcClient::from_config(&config.network)
        │
        ├─ inspect_contract(client, id)  ──ok──▶ wasm_hash (on-chain)
        │        └─ Err(ContractNotFound) → stderr "Error: contract <id> not found", exit 1
        │        └─ other Err            → stderr "Error fetching contract: <e>", exit 1
        │
        ├─ StorageAnalyzer::new(client).inspect_contract_storage(id)
        │        └─ Err → stderr, exit 1
        │
        ├─ (if --wasm) compare local_hash == wasm_hash → verified: bool
        │
        ├─ derive_verdict(wasm_hash, storage_report, verified_opt) → (health, reasons[])
        │
        └─ render ContractHealthReport (pretty | json)
```

All RPC calls are **read-only** and **already implemented**. The only new network behavior is *calling two existing functions sequentially* (one `inspect_contract`, one `StorageAnalyzer` which itself issues `get_ttl_info`). No new `getLedgerEntries` semantics.

---

## 9. Public API Changes

**None in any library crate.** `sdkt-rpc`, `sdkt-storage`, `sdkt-wasm`, `sdkt-core`, `sdkt-xdr`, `sdkt-audit` are untouched.

`sdkt-cli` gains (additive only):
- `Commands::Health { contract, wasm, network, format }` — new enum variant.
- `struct ContractHealthReport { … }` — new (CLI-local, `#[derive(serde::Serialize)]`).
- `async fn contract_health(client, contract_id, local_wasm: Option<&[u8]>, network) -> Result<ContractHealthReport, String>`.
- `fn derive_verdict(...) -> HealthVerdict` — pure helper.

No existing command, flag, struct, or function signature changes. Binary `--version` and help output gain one entry. Fully backward compatible.

---

## 10. Internal Implementation Plan

All in `crates/sdkt-cli/src/main.rs` (same file as M22 `verify`):

1. **Add `Commands::Health`** variant (after `Verify`), mirroring M22's arg attributes:
   ```rust
   /// Unified read-only contract posture report (M23)
   Health {
       #[arg(short, long, value_name = "CONTRACT_ID")] contract: String,
       #[arg(long, value_name = "WASM")] wasm: Option<String>,
       #[arg(short, long, default_value = "testnet")] network: String,
       #[arg(short, long, default_value = "pretty")] format: String,
   },
   ```

2. **Add `ContractHealthReport`** struct (serde Serialize, fields below in §12). Reuse `StorageReport` and `TtlInfoSummary` by embedding/serializing their existing fields (do **not** redefine them).

3. **Add `derive_verdict`** pure function:
   - Inputs: `verified: Option<bool>`, `expiring_soon: usize`, `total_entries: usize`, `contract_not_found: bool` (handled before this), `wasm_present: bool`.
   - Rules (transparent, testable):
     - If `verified == Some(false)` → `Critical` (deployed != built).
     - Else if `expiring_soon > 0` → `AtRisk` (entries near TTL expiry).
     - Else if `total_entries == 0` → `AtRisk` (empty contract — unusual).
     - Else → `Healthy`.
   - Returns `(health: &'static str, reasons: Vec<String>)`.

4. **Add `contract_health`** orchestrator:
   - Offline hash of `--wasm` first (fail-fast, reuse M22 error wording).
   - `inspect_contract` → on-chain `wasm_hash`.
   - `StorageAnalyzer::new(client).inspect_contract_storage(id)` → `StorageReport`.
   - If `--wasm`, call existing `verification_outcome(wasm_hash, Some((local_hash, local_size)))` to get `verified`.
   - `derive_verdict(...)`.
   - Build `ContractHealthReport`.

5. **Add `Commands::Health` match arm** in `main`: build client from `.sdkt.toml`, call `contract_health`, print pretty or `serde_json::to_string_pretty`. Error branch mirrors M22 (`process::exit(1)` with actionable message).

No edits to `Cargo.toml` (the `serde` derive dependency already added in M22 is sufficient; `serde_json` already present).

---

## 11. Error Model

| Condition | Message (stderr) | Exit |
|-----------|------------------|------|
| Missing `--contract` | clap: `error: a value is required for '--contract <CONTRACT_ID>'` | 2 |
| Invalid `--format` | `Invalid format 'x'. Use 'json' or 'pretty'.` | 1 |
| `--wasm` file missing | `Error reading WASM file <path>: <io>` | 1 |
| `--wasm` not valid WASM | `Error: <path> is not valid WASM` | 1 |
| Contract not found on-chain | `Error: contract <id> not found on <network>` | 1 |
| RPC / network error | `Error fetching contract: <e>` | 1 |
| Storage analysis error | `Error analyzing storage: <e>` | 1 |

No panics on user input. Same discipline as M22.

---

## 12. JSON Schema

```json
{
  "contract_id": "CABCDEFGHIJKLMNOP",
  "network": "testnet",
  "health": "healthy",            // "healthy" | "at_risk" | "critical"
  "verified": true,               // bool; null when --wasm omitted
  "on_chain_wasm_hash": "3b9f...c2",
  "local_wasm_hash": "3b9f...c2", // null when --wasm omitted
  "local_wasm_size_bytes": 12345, // null when --wasm omitted
  "storage": {
    "total_entries": 12,
    "instance_entries": 1,
    "persistent_entries": 9,
    "temporary_entries": 2,
    "other_entries": 0,
    "ttl": {
      "minimum_ttl": 518400,
      "maximum_ttl": 518400,
      "average_ttl": 518400,
      "expiring_entries_count": 0,
      "estimated_rent_cost": 240000
    }
  },
  "reasons": []                   // human-readable verdict reasons (empty when healthy)
}
```

Field notes:
- `health` uses `snake_case` enum serialization (`health`/`at_risk`/`critical`) for stable machine parsing.
- `verified` / `local_wasm_hash` / `local_wasm_size_bytes` are `Option` → `null` when `--wasm` omitted (matches M22 OnChainOnly behavior).
- The `storage` block reuses `StorageReport` field names exactly (no rename), so existing `StorageAnalyzer` consumers see a familiar shape.
- `reasons` is always present (empty `[]` when `Healthy`).

---

## 13. Pretty Output Examples

See §6 for the two canonical examples (HEALTHY and AT RISK). Additional:

**Missing `--wasm` (OnChainOnly posture):**
```
Contract Health Report
=======================
Contract ID : CABCDEFGHIJKLMNOP
Network     : testnet
Health      : HEALTHY

On-chain WASM : 3b9f...c2
Storage:
  Total Entries: 7
    Instance:    1
    Persistent:  5
    Temporary:   1
TTL:
  Min TTL:        518400
  Max TTL:        518400
  Average TTL:    518400
  Expiring Soon:  0
  Est. Rent Cost: 140000 stroops

Verdict: Contract posture is healthy. No local WASM supplied; verification skipped.
```

**Critical (WASM mismatch):**
```
Health      : CRITICAL

Verdict: On-chain WASM does NOT match the supplied local file. Rebuild and
redeploy, or confirm you are comparing the correct artifact.
```

---

## 14. Testing Strategy

**Unit tests** (`#[cfg(test)] mod m23_tests` in `main.rs`):
- `derive_verdict_healthy` — verified Some(true), 0 expiring, >0 entries → `healthy`.
- `derive_verdict_at_risk_expiring` — 0 expiring? → no; 2 expiring → `at_risk`.
- `derive_verdict_critical_mismatch` — verified Some(false) → `critical` (regardless of TTL).
- `derive_verdict_at_risk_empty` — total_entries == 0 → `at_risk`.
- `derive_verdict_onchain_only` — verified None (no `--wasm`) and 0 expiring → `healthy`.
- `health_report_json_schema` — build a `ContractHealthReport` and assert the JSON keys from §12 (including `null` for omitted `--wasm` fields, `health` snake_case).

**Integration tests** (`tests/health_integration_test.rs`, hermetic / offline-reachable):
- `test_cli_health_missing_contract_arg` — `sdkt health` with no `--contract` → failure.
- `test_cli_health_invalid_format` — `--format bogus` → exit 1, stderr `Invalid format`.
- `test_cli_health_missing_wasm_file` — `--wasm /nope` → exit 1, stderr `Error reading WASM`.
- `test_cli_health_invalid_wasm` — `--wasm <bad>` → exit 1, stderr `not valid WASM`.
- `test_cli_health_json_flag_accepted` — `--format json` with bad wasm → exit 1, stderr `not valid WASM` (proves flag parsed + JSON path reachable).
- `test_cli_health_onchain_error_path` — valid minimal wasm + bogus contract id → reaches RPC, exits 1 (exercises the on-chain fetch + error branch).

**End-to-end (requires RPC, not hermetic — documented, not in CI):**
- A live contract with `--wasm` matching → `health: "healthy"`, `verified: true`.
- A live contract with mismatched `--wasm` → `health: "critical"`.
These are covered by the pure `derive_verdict` + `verification_outcome` unit tests; the live network leg is already validated by M22's `verify` integration tests.

---

## 15. Cross-platform Considerations

- Identical to M22: pure Rust, no platform-specific APIs in the new code. `fs::read` for the optional local WASM is portable. `StorageAnalyzer` / `inspect_contract` are network-only and OS-agnostic.
- CI matrix (Linux/macOS/Windows, MSRV 1.88.0) unchanged — M23 adds no platform-specific dependency.
- No path normalization needed beyond the user-supplied `--wasm` (consistent with M21/M22).

---

## 16. Security Considerations

- **Read-only.** No `simulate`/`submit`/`extend`/`restore`. Cannot mutate chain state. Same trust boundary as `inspect`/`storage analyze`.
- **Panic paths:** none on user input. `fs::read` and `serde_json::to_string_pretty` use `unwrap_or_else` + `process::exit(1)`; `DevKitConfig::from_file(...).unwrap_or_default()` is safe. No raw `.unwrap()` on network/parse results.
- **Path handling:** `--wasm` read directly via `fs::read`; no traversal synthesis. Accepted local-CLI design (same as M21/M22).
- **Malformed WASM:** parsed offline via `sdkt-wasm::parse_metadata` (streaming `wasmparser` + bounded XDR). Bad bytes → `WasmError::Parse` → `not valid WASM`, exit 1. No execution.
- **Malformed RPC responses:** handled by `sdkt-rpc` (`RpcError`) → surfaced to the error branch, exit 1.
- **DoS / resource:** local file fully read into memory (same ~16 MB optional cap deferred as in M21/M22); only two read-only RPC calls (no bytecode download). No amplification.
- **No secrets, no code execution, no eval.** Clean.

---

## 17. Performance Considerations

- **RPC calls:** exactly two read-only calls — `inspect_contract` (1 `getLedgerEntries`) + `StorageAnalyzer.inspect_contract_storage` (internally `get_ttl_info`, 1 `getLedgerEntries`). No `getWasm`/`getCode` bytecode download.
- **Hashing:** one SHA-256 over the local `--wasm` (only if supplied), via `parse_metadata`.
- **No duplicate parsing:** `StorageReport` and `ContractInspection` are consumed as-is; the comparison reuses M22's `verification_outcome` (no second parse of the WASM).
- **Allocations:** one `ContractHealthReport` built per invocation; `format!` only for non-empty `reasons`. Negligible.
- **Network:** sequential, not parallel — acceptable (two cheap reads). A future optimization could `join!` the two awaits; out of scope for M23 (keep it simple and reviewable).

---

## 18. Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `inspect_contract` returns only `wasm_hash` + keys, not full storage summary | Already the case | Low — `StorageAnalyzer` supplies the storage/TTL data independently | M23 composes both; no dependency on `inspect_contract` for storage |
| `--network` label vs actual endpoint mismatch (config-driven) | Medium | Low — cosmetic only | Documented (same as M22); `--network` labels the report, endpoint from `.sdkt.toml` |
| TTL thresholds (30-day "expiring soon") too strict/loose | Low | Low | Constants centralized in `derive_verdict`; easy to tune; unit-tested |
| Verdict heuristic considered "opinionated" | Low | Low | Reasons are explicit and machine-readable; users can ignore `health` and read raw fields |
| Two sequential RPC calls slow on high-latency networks | Low | Low | Each is a single read; total < 1s typical; parallelization deferred |

---

## 19. Future Extensions

- **FE1.** Parallelize the two RPC calls (`tokio::join!`) — trivial once stable.
- **FE2.** Add `--history` TTL trend by sampling `getTtlInfo` over time (requires new storage; out of scope per NG5).
- **FE3.** `sdkt health --batch <file>` to scan many contracts (CI posture dashboard).
- **FE4.** Fold in `sdkt events` recent-activity signal (rate of recent events) into the verdict.
- **FE5.** `--watch` mode that re-runs and diffs posture (alerts on drift).
- **FE6.** Post-1.0 mainnet/SCF alignment: annotate health with network-specific thresholds.

---

## 20. Definition of Done

- [ ] `Commands::Health` added; `sdkt health --contract <ID> [--wasm <f>] [--network <n>] [--format <p|j>]` works.
- [ ] Reuses `sdkt-rpc::inspect_contract`, `sdkt-storage::StorageAnalyzer`, and (if `--wasm`) `sdkt-wasm::parse_metadata` + M22 compare — **no new RPC, no new crate**.
- [ ] `derive_verdict` is a pure, unit-tested function with `healthy`/`at_risk`/`critical` rules.
- [ ] JSON schema (§12) and pretty output (§6/§13) implemented and verified.
- [ ] Exit codes match §5/§11 (0 = report produced; 1 = operational failure).
- [ ] Unit tests (`m23_tests`) + integration tests (`health_integration_test.rs`) passing.
- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace` all green.
- [ ] README.md, CHANGELOG.md, ROADMAP.md, docs/cli.md updated.
- [ ] No changes to any library crate's public API; fully backward compatible.
- [ ] No `Cargo.toml` edits required (M22 already added `serde` derive; `serde_json` present).
- [ ] Committed as `feat(cli): M23 Contract Health Report`; not pushed unless instructed.
