# M22 — Contract Verification (Design Plan)

**Status:** Design only (no code changes)
**Target version:** v1.4.0 (post M21)
**Author:** IRONCLAW design pass
**Last updated:** 2026-08-05

---

## 1. Problem Statement

Soroban developers frequently deploy a WASM binary and later need to prove
that the **deployed** contract bytecode matches the **local** artifact they
intended to ship. Today `sdkt` can:

- inspect an on-chain contract and return its on-chain WASM hash (`sdkt inspect`),
- fetch and parse WASM metadata from RPC (`sdkt wasm metadata`),
- parse a local WASM offline (`sdkt wasm inspect`),

but there is **no single command** that compares the two and gives a clear
`match: true/false` verdict. Teams resort to manual hashing, copy-paste, and
ad-hoc scripts — error-prone and not auditable.

M22 closes this gap with `sdkt verify`.

---

## 2. Goals

- Add `sdkt verify` to compare a deployed contract's on-chain WASM hash
  against a locally supplied WASM file (or against the network's own record).
- Perform **all local hashing offline** using `sdkt-wasm::parse_metadata`.
- Fetch the on-chain contract code exclusively through the **existing RPC
  layer** (`sdkt-rpc`), reusing `inspect_contract` and `get_wasm_metadata`.
- Produce machine-readable (`--format json`) and human-readable reports.
- Fail safely with actionable error messages; never panic on malformed input.
- Reuse existing crates; introduce no new crate.

---

## 3. Non-Goals

- **No re-upload / deployment.** Verification is read-only.
- **No semantic diff.** M22 compares WASM *hashes*, not function-level ABI
  diffs (that is `sdkt diff`, already exists).
- **No custom RPC calls.** Strictly reuse `sdkt-rpc` public functions.
- **No signature / authorization checks.** We verify bytecode identity, not
  who deployed it.
- **No local WASM → network fetch** for arbitrary hashes (only the
  contract's own on-chain code is fetched).

---

## 4. User Stories

1. As a deployer, I run `sdkt verify --contract C... --wasm contract.wasm`
   to confirm the deployed bytecode equals my local artifact before
   announcing a release.

2. As a auditor, I run `sdkt verify --contract C... --network testnet`
   (no local file) to fetch the on-chain hash and confirm the lookup works
   and returns a consistent report (self-verify / smoke test).

3. As a CI pipeline, I call `sdkt verify ... --format json` and grep/parse
   `match: true` to gate a promotion job.

4. As a reviewer, when hashes differ, I get a clear explanation naming the
   two hashes and a hint about rebuild/reupload.

---

## 5. CLI UX Proposal

New top-level command `verify` on `sdkt-cli` (sibling to `inspect`, `wasm`,
`diff`).

```
sdkt verify \
    --contract <CONTRACT_ID> \
    --wasm contract.wasm

sdkt verify \
    --contract <CONTRACT_ID> \
    --network testnet

sdkt verify \
    --contract <CONTRACT_ID> \
    --wasm contract.wasm \
    --format json
```

Argument spec (clap):

| Flag | Required | Default | Notes |
|------|----------|---------|-------|
| `--contract` | yes | — | Stellar contract ID (`C...`). |
| `--wasm` | no | — | Path to local WASM. Offline hashed. |
| `--network` | no | from `.sdkt.toml` / `testnet` | Network for RPC fetch. |
| `--format` | no | `pretty` | `pretty` \| `json`. |

Behavior matrix:

- `--wasm` present → compute local hash offline, fetch on-chain hash, compare.
- `--wasm` absent → fetch on-chain hash only, report it (no comparison verdict,
  `match` = `null`). This is the "smoke test" mode from user story 2.

Exit codes:
- `0` → verification completed (regardless of match result; `match:false` is
  a successful *verification* with a negative result, not a tool error).
- `1` → operational failure (bad contract ID, RPC error, unreadable file,
  invalid WASM).

---

## 6. Architecture

Crate boundaries (unchanged, reused):

```
sdkt-cli   → adds Commands::Verify { .. }   (parses args, formats output)
sdkt-rpc   → inspect_contract(), get_wasm_metadata()  (network fetch)
sdkt-wasm  → parse_metadata()  (offline local hash + size)
sdkt-xdr   → encode_ledger_key, extract_wasm_hash, extract_wasm_bytecode
sdkt-core  → DevKitConfig, NetworkConfig, OutputFormat
```

No new crate. No logic in `sdkt-cli` beyond argument parsing and output
formatting. All hashing and comparison math lives in `sdkt-wasm` /
`sdkt-rpc` as it does today.

### New reusable helper (optional, in `sdkt-rpc` or `sdkt-cli` glue)

A small `verify_contract` async function (placed in `sdkt-cli` glue or a
new `sdkt-rpc::verify` module) orchestrates:

```rust
pub async fn verify_contract(
    client: &SorobanRpcClient,
    contract_id: &str,
    local_wasm: Option<&[u8]>,
) -> Result<VerificationReport, RpcError>;
```

`VerificationReport` is a plain serializable struct (see §10).

---

## 7. Data Flow

**Case A — local WASM provided:**

1. CLI reads `--wasm` file bytes (`fs::read`), offline.
2. `sdkt_wasm::parse_metadata(&local_bytes)` → `local_hash`, `local_size`.
3. CLI builds `SorobanRpcClient::from_config(&network)`.
4. `inspect_contract(&client, contract_id)` → `ContractInspection`
   containing `wasm_hash` (on-chain).
5. Compare `local_hash == on_chain_hash`.
6. Format report.

**Case B — no local WASM (smoke test):**

1. Steps 3–4 only.
2. Report `on_chain_hash`, `match = null`, status `OnChainOnly`.

Both paths reuse `inspect_contract` exactly as `sdkt inspect` does today.
No new RPC method is added.

---

## 8. Public API Changes

**`sdkt-cli` (additive):**
- `enum Commands { Verify { contract, wasm, network, format }, ... }`

**`sdkt-rpc` (additive, optional):**
- `pub async fn verify_contract(client, contract_id, local_wasm: Option<&[u8]>) -> Result<VerificationReport, RpcError>`
  (thin orchestration over existing `inspect_contract` + `parse_metadata`).

**`sdkt-wasm` (unchanged):** `parse_metadata` already returns `hash` and
`size_bytes`. No change needed.

**`sdkt-xdr` (unchanged):** `extract_wasm_hash`, `encode_ledger_key` already
used by `inspect_contract`. No change needed.

Backward compatibility: fully preserved — `verify` is a new command; no
existing command, flag, or struct is modified.

---

## 9. Error Model

All errors are surfaced through the existing `RpcError` (network) and
`sdkt_wasm::WasmError` (local parse) types, mapped to a clear CLI message
and `process::exit(1)`.

| Condition | Source | User message |
|-----------|--------|--------------|
| Contract ID invalid format | `extract_wasm_hash`/`inspect_contract` | `Error: invalid contract ID` |
| RPC / network failure | `RpcError` | `Error fetching contract from <network>: <e>` |
| Contract not found on-chain | `RpcError::ContractNotFound` | `Error: contract <id> not found on <network>` |
| Local WASM unreadable | `fs::read` | `Error reading WASM file <path>: <e>` |
| Local WASM not valid WASM | `WasmError::Parse` | `Error: <path> is not valid WASM` |
| Local WASM empty | `WasmError::Empty` | `Error: <path> is empty` |

No `unwrap()` on user-controlled paths. File read and parse use
`unwrap_or_else` + `process::exit(1)` exactly like the M21 `Inspect` arm.

---

## 10. JSON Output Schema

```json
{
  "contract_id": "CABCDEFG...",
  "network": "testnet",
  "on_chain_wasm_hash": "3b9f...",
  "local_wasm_hash": "3b9f..." ,   // null when --wasm omitted
  "local_wasm_size_bytes": 12345,   // null when --wasm omitted
  "match": true,                     // null when --wasm omitted
  "verification_status": "Verified", // Verified | Mismatch | OnChainOnly | Error
  "explanation": ""                 // populated when mismatch or on-chain-only
}
```

`verification_status` enum values:
- `Verified` — hashes equal.
- `Mismatch` — both present, differ.
- `OnChainOnly` — no local WASM supplied; only on-chain hash reported.
- `Error` — operational failure (not emitted in normal JSON; error goes to
  stderr + exit 1).

---

## 11. Human-Readable Output

```
Contract Verification Report
============================
Contract ID : CABCDEFG...
Network     : testnet
On-chain WASM: 3b9f...
Local WASM   : 3b9f...   (12345 bytes)
Match        : YES
Status       : Verified
```

Mismatch example:

```
Contract Verification Report
============================
Contract ID : CABCDEFG...
Network     : testnet
On-chain WASM: 3b9f...
Local WASM   : a1c2...   (12345 bytes)
Match        : NO
Status       : Mismatch

The deployed bytecode does NOT match the local file.
On-chain : 3b9f...
Local    : a1c2...
Rebuild and redeploy, or confirm you are comparing the correct artifact.
```

On-chain-only example:

```
Contract Verification Report
============================
Contract ID : CABCDEFG...
Network     : testnet
On-chain WASM: 3b9f...
Match        : N/A (no local WASM provided)
Status       : OnChainOnly
```

---

## 12. Testing Strategy

**Unit (sdkt-rpc or sdkt-cli glue, `#[cfg(test)]`):**
- `verify_contract` with a mocked client returning a known hash, local bytes
  with matching hash → `match: true`.
- `verify_contract` with mismatched local bytes → `match: false`.
- `verify_contract` with `local_wasm: None` → `OnChainOnly`.

**Integration (sdkt-cli/tests/verify_integration_test.rs):**
- `test_cli_verify_missing_contract_arg` → failure (clap required arg).
- `test_cli_verify_invalid_wasm` → stderr contains `not valid WASM`, exit 1.
- `test_cli_verify_missing_wasm_file` → stderr contains `Error reading WASM`,
  exit 1.
- `test_cli_verify_json_shape` → offline WASM (minimal valid blob) compared
  against a fake on-chain hash via a mocked/in-process client OR a recorded
  fixture; assert JSON contains `contract_id`, `on_chain_wasm_hash`,
  `local_wasm_hash`, `match`, `verification_status`.
- `test_cli_verify_human_output` → assert `Match : YES/NO` present.

**Regression:** existing `wasm inspect`, `inspect`, `wasm metadata`,
`diff` tests must stay green. `verify` is purely additive.

Note: network-dependent paths are covered by unit tests with injected
clients; the CLI integration tests use minimal local WASM blobs (no live
RPC) to stay hermetic and cross-platform.

---

## 13. Cross-Platform Considerations

- Local hashing is 100% offline and platform-independent (`sha2` +
  `wasmparser`), identical to M21 `Inspect` behavior.
- RPC fetch uses the same `sdkt-rpc` client already proven on Linux/macOS/
  Windows in CI.
- No `std::os::unix` usage introduced. All file reads use `std::fs::read`.
- Exit codes and stdout/stderr semantics are POSIX-consistent; Windows CI
  already exercises the same CLI harness.

---

## 14. Security Considerations

- **No secrets:** verification only reads public contract state and a local
  file the user already possesses.
- **No code execution:** local WASM is parsed, never executed. `parse_metadata`
  streams via `wasmparser` with bounded XDR decode (`Limits::none()` is
  already used in `sdkt-wasm`; consider `Limits::none()` vs a tighter limit —
  see §16).
- **Panic surface:** identical to M21 — file read and parse use
  `unwrap_or_else` + `process::exit(1)`; no user input reaches an `unwrap`.
- **Supply-chain:** hashes compared as hex strings; no eval, no dynamic
  dispatch on untrusted data.
- **Path handling:** `--wasm` is read directly; no traversal beyond the
  user-named path (local dev tool, accepted design).

---

## 15. Performance Considerations

- Local hash: single `Sha256` over the file bytes + one `wasmparser` walk.
  Same cost as `sdkt wasm inspect`. Negligible for <2 MB Soroban WASM.
- On-chain fetch: exactly one `getLedgerEntries` call via `inspect_contract`
  (no bytecode download needed — only the hash is required). M22 should
  prefer `inspect_contract` (returns hash directly) over `get_wasm_metadata`
  (which downloads and re-parses bytecode) to avoid an unnecessary large
  transfer. **Optimization:** if only the hash is needed, use
  `inspect_contract` and do NOT call `get_wasm_metadata`.
- No duplicate parsing: local hash from `parse_metadata` only; on-chain hash
  from `inspect_contract` only. No second walk.
- Memory: file read into memory once (same F-02 caveat as M21 — acceptable
  for a local CLI; optional size cap can be added later).

---

## 16. Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `inspect_contract` returns hash but contract has no WASM (edge case) | Low | Wrong verdict | Treat empty/missing hash as `Error: no WASM on contract` |
| XDR decode of on-chain entry unbounded | Low | Memory spike | Reuse existing `Limits`; optionally tighten in `sdkt-xdr` later |
| User passes wrong network | Med | False mismatch | Report `network` prominently; document in help |
| Large local WASM OOM | Low | Cli crash | Optional 16 MB cap (deferred, same as M21 F-02) |
| RPC rate limit / timeout | Med | Transient failure | `sdkt-rpc` already has retry/timeout; surface error clearly |

---

## 17. Future Extensions

- **ABI diff gate:** after hash match, optionally run `sdkt diff` to confirm
  no *semantic* regression even if bytecode differs for benign reasons
  (compiler version).
- **Batch verify:** `sdkt verify --manifest deployments.toml` to verify many
  contracts in one run (CI matrix).
- **Pinned verification:** store expected hash in `.sdkt.toml`
  (`[verify] contract = "hash"`) and fail CI if drift detected.
- **WASM-only mode:** `sdkt verify --wasm a.wasm --wasm b.wasm` to compare
  two local artifacts (offline sibling of `diff`).

---

## 18. Definition of Done

- [ ] `sdkt verify` implemented with `--contract`, `--wasm`, `--network`,
      `--format` flags.
- [ ] Local WASM hashed fully offline via `sdkt-wasm::parse_metadata`.
- [ ] On-chain hash fetched only via `sdkt-rpc::inspect_contract` (no new
      RPC method).
- [ ] JSON schema from §10 emitted with `--format json`; human report from §11
      with default `pretty`.
- [ ] `match`, `verification_status`, and `explanation` populated correctly
      for Verified / Mismatch / OnChainOnly.
- [ ] All error cases from §9 mapped to clear stderr messages + exit 1.
- [ ] Unit tests for `verify_contract` (match / mismatch / on-chain-only).
- [ ] CLI integration tests in `verify_integration_test.rs` (invalid WASM,
      missing file, JSON shape, human output).
- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features
      -- -D warnings`, `cargo test --workspace` green.
- [ ] CI green on Linux, macOS, Windows, MSRV.
- [ ] README / CHANGELOG / ROADMAP updated; `docs/cli.md` gains a `verify`
      section.
- [ ] No existing public API modified; no new crate introduced.
- [ ] Backward compatible; `verify` is purely additive.

---

**End of M22 design.** No source modified, no commit created.
