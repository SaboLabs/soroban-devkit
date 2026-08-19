# EXTERNAL_AUDIT_2026-08-19.md

Local, read-only. Clones under `/tmp/sdkt-probe/` (not the sdkt repo).
Binary: `/home/ubuntu/soroban-devkit/target/debug/sdkt` (`sdkt 2.5.0`).
Command family: `sdkt audit <path/to/src/lib.rs> [--format json]`.
**TOOL FINDINGS, not vulnerabilities.** No issues opened.

Exclude: SaboLabs/*, naninu123/*, Shadow-MMN/soroban-devkit, sorocore/soroban-devkit.

## Candidates

| # | Repo | Why chosen | Stars | Issues | License | Fit for `sdkt audit` |
|---|---|---|---|---|---|---|
| 1 | NethermindEth/stellar-private-payments | Official-adjacent privacy contracts, active 2026-08-19 | 55 | 80 open | Apache-2.0 | Yes — `contracts/*/src/lib.rs` |
| 2 | water-credits/water-credits-contracts | Multi-contract Soroban protocol, MIT | 7 | 9 open | MIT | Yes |
| 3 | Vero-protocol/vero-core-contracts | On-chain protocol, active | 7 | 79 open | unknown | Partial — root `src/lib.rs` is a module facade |
| 4 | Lafiya-xyz/Lafiya-contract | Attestation registries | 8 | 33 open | MIT | Yes |
| 5 | Stellar-VaultLink/invofi-contracts | Invoice/financing contracts | 8 | 130 open | MIT | Yes |

Skipped as issue targets: `stellar/rs-soroban-sdk` (SDK not a dapp contract), Tollcraft linter (peer tool), 0-star name farms.

## Results (compact)

AUTH-001/002/003 on **getters, `has_admin`/`read_admin`/`is_paused`, and helpers** are false positives of the current syn heuristic (name looks privileged; body is a storage read or delegates `require_auth()`). MOVE-001 on `Env`/`Address` locals is noise for typical Soroban patterns.

### 1. NethermindEth/stellar-private-payments

| Crate | AUTH-001 (raw) | Independent check | Actionable for maintainer? |
|---|---|---|---|
| asp-membership `update_admin` | critical | Body calls `soroban_utils::update_admin` which does `admin.require_auth()` (`contracts/soroban-utils/src/utils.rs:24`) | **No** — inter-module auth |
| asp-non-membership `update_admin` | critical | same helper | **No** |
| pool `update_admin` | impl in `pool.rs`; same helper | **No** |
| pool-core / pool-gvk / pool `lib.rs` | 0 | n/a | No issue |
| Others | MOVE-001 only | noise | No |

**Issue? No.** Filing "your AUTH is in a helper" wastes an 80-issue inbox.

### 2. water-credits/water-credits-contracts

Raw AUTH-001 volume is high (`has_admin`, `read_admin`, `is_paused`, `initialize`, getters). Independent check:

- `credit_token`: `has_admin`/`read_admin`/`is_paused` are **private helpers**, not entrypoints. `propose_admin`/`set_minter`/`accept_admin` call `require_auth()`.
- `governance`: `propose`/`vote`/`execute`/`update_config`/`transfer_admin` call `require_auth()`. `do_pause` / `mint_credits_respecting_cap` / `penalize_non_revealers` are internal (`fn`, not public entry) or called from already-authenticated flows.

**Issue? No.** A wall of getter AUTH-001 would be spam, not help.

### 3. Vero-protocol/vero-core-contracts

`sdkt audit src/lib.rs` → **0 findings** (file is `mod` declarations). `verification/src/lib.rs` → 0. Analyzer does not recurse modules. **Not a useful first-run for this layout.**

**Issue? No.**

### 4. Lafiya-xyz/Lafiya-contract

AUTH-001 on `get_admin` / `is_paused` / `admin` / `require_not_paused` — accessors/guards. Mutators (`initialize`, `propose_admin`, `pause`, `attest`, `add_attester`) **do** `require_auth()`.

**Issue? No.**

### 5. Stellar-VaultLink/invofi-contracts

AUTH-001 on `get_admin` / `contract_is_paused`. Mutators `transfer_admin`/`pause`/`unpause` go through `assert_admin` → `caller.require_auth()`.

**Issue? No.**

## Valid technical findings (for *sdkt*, not for targets)

1. **AUTH-001 does not follow `require_auth` across helper functions / other modules.** False positives on Nethermind `update_admin` wrappers.
2. **AUTH-001 flags private getters** (`read_admin`, `has_admin`, `is_paused`) as if they were privileged entrypoints.
3. **MOVE-001 floods** on `Env` and `Address` (Soroban pass-by-ref / clone patterns).
4. **No workspace recursion** — Vero-style `mod contracts` roots look "clean" while real entrypoints live elsewhere.

These are **sdkt quality limitations**, useful for our backlog, **not** reasons to open issues on those five repos.

## Issue count

**0 valid external issues.** Acceptable per mission ("0 valid issues is an acceptable outcome").

WASM inspect: no committed `.wasm` outside `target/` in the five clones → skipped.

## Classification

| Target | Local run | Maintainer-useful issue | Class |
|---|---|---|---|
| Nethermind | yes | no | LOCAL EVALUATION ONLY |
| water-credits | yes | no | LOCAL EVALUATION ONLY |
| Vero | yes (vacuous) | no | NO FIT for single-file audit |
| Lafiya | yes | no | LOCAL EVALUATION ONLY |
| Invofi | yes | no | LOCAL EVALUATION ONLY |

OUTREACH: **0** (nothing sent).
