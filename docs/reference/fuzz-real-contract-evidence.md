# Real-Contract Fuzz Evidence

## 1. Purpose

This page is **evidence that the `sdkt-fuzz` v2.7.0 pipeline actually runs on
external Soroban WASM** — not a security audit, not a vulnerability-discovery
proof, and not a completeness claim.

Two independent pilots were executed offline against contracts built from the
official [`stellar/soroban-examples`](https://github.com/stellar/soroban-examples)
repository (HEAD `03d42aa6b973dcf3a453a99d0c6a6e8d25a196e2`). Every claim below
is backed by a reproducible artifact: a WASM SHA-256, a canonical artifact
hash, a replay verdict, or a byte-identical rerun.

## 2. Environment

| Component | Version / value |
|---|---|
| `sdkt-fuzz` | v2.7.0 |
| Pipeline PR | #285 — deterministic Soroban fuzzing and replay |
| Auth fix PR | #286 — correct auth classification and replay |
| Host engine | `soroban-env-host` 28.0.2 (in-memory, offline) |
| XDR | `stellar-xdr` 28.0.0 |
| Build toolchain | `stellar contract build` 28.1.0 (`wasm32v1-none`) |
| External source | `stellar/soroban-examples` @ `03d42aa6` |
| Execution model | offline, in-memory Host only — no network, no ledger, no transactions |

## 3. Pilot #1 — External Contracts (no `require_auth`)

Four official example contracts were compiled with the Stellar CLI and fuzzed
through the public CLI and library API.

| Target | WASM SHA-256 |
|---|---|
| `increment` | `36bc3b311c3780c847e5724d9dc0d5f160af22d476535a2881ea9155d2049dce` |
| `custom_types` | `2f6f5657cb8ec38227ba636de221aed75ac31ac6db9614b710f73ae5686ebd40` |
| `events` | `33084e80e278a22703f6bd4f70cc3e70d4bd3718468ab5401202c36f36a0ef7c` |
| `errors` | `c958ec5683d4998d5e3edfcfb3e07a2d33978579197bceb0394bf3d8e0ef99a8` |

Each WASM is byte-identical to the corresponding
`stellar/soroban-examples/<dir>/target/wasm32v1-none/release/*.wasm` build
output.

### Evidence

- **ContractSpec parsing** — `selectable=["increment"]` (increment, events,
  errors) and `selectable=["get_state","increment"]` (custom_types);
  `skipped=[]`. The `Address` parameter type is supported by the generator.
- **Typed generation + mutation** — deterministic per-case argument vectors
  with typed semantic mutation (e.g. `u32(4294967295) -> u32(0)`).
- **Real host execution** — every observation carries live budget counters
  (e.g. `consumed_cpu: 306105`, `remaining_cpu: 99693895`) and a state
  footprint in XDR. Replay re-executes through `Executor`; it does not read
  metadata.
- **Stateful sequence** (`custom_types`, `seq_direct`):
  `get_state → 0`, `increment(5) → U32(5)`, `increment(7) → U32(12)`,
  `get_state → count=12, last_incr=7`, `carried_instance_storage_entries=1`.
- **ExpectedError classification** (`errors` contract, `MAX=5`): steps 0–4
  `Returned(U32(1..5))` → `Finding(ReturnMismatch)`; steps 5–6
  `ContractError{Contract,1}` → `ExpectedError` (exactly at `MAX+1`).
- **Minimization** — 45/45 `preserved=true`; complexity `11..43 → 11`.
- **Replay** — 45/45 `REPRODUCED`.
- **Deterministic rerun** — `campaign-increment.json` == rerun
  (SHA-256 `54a5c9b86d4c582d4f0cce7d257d6889afbfdd9a6165d5b95538e0d05137c918`);
  `campaign-ct-declared.json` == rerun; `artifacts-ct` == `artifacts-ct2`
  byte-identical across separate processes.

### Canonical hash

The canonical artifact identity is the `artifact_hash` field inside the
artifact JSON (SHA-256 of the canonical JSON bytes). Example:

```
0ffdcd2395106c3c5a379cd6d482dbefc941efad95263f9fb57e3cf99c36c761
```

This is **not** the SHA-256 of the file on disk (which includes a trailing
newline and is a different value).

## 4. Pilot #2 — Real Auth (`require_auth`)

Target: `stellar/soroban-examples/auth` — `IncrementContract` with a single
exported function `increment(user: Address, value: u32) -> u32`.

**Source evidence** — `auth/src/lib.rs:37`:

```rust
user.require_auth();
```

WASM SHA-256:

```
cae53933dc47d5c3a77dda027c2e50aeadc2fb2f5721770b164a3901111542c8
```

(926 bytes, `stellar contract build` 28.1.0.)

### Auth matrix

Seeded baseline `Counter(source) = 0`, 3-step sequence
`increment(source, 5)` × 3, `user = source_address()`:

| Mode | Step 0–2 | Counter after |
|---|---|---|
| `NoAuth` | `ContractError{Auth,6}` | `0` |
| `CorrectAuth` | `Returned(U32(5))`, `Returned(U32(10))`, `Returned(U32(15))` | `15` |
| `WrongAuth` | `ContractError{Auth,6}` | `0` |

Oracle classification (`Oracle::with_auth_mode`):

| Mode | declared `Error(Auth,6)` | declared `Success` |
|---|---|---|
| `NoAuth` | `ExpectedError` | `Finding(UnexpectedError)` |
| `CorrectAuth` | `Finding(ReturnMismatch)` | `Pass` |
| `WrongAuth` | `ExpectedError` | `Finding(UnexpectedError)` |

Replay: single-step 12/12 `REPRODUCED`; multi-step `CorrectAuth` artifact
`REPRODUCED` under the #286 lineage (canonical
`a16041e410b4bef28d959f06ed827a20047ecffcc2a31a51be8c6c713e72e36a`,
observation `092451aca38117dbfa90bbaf68f2c63586e072d9d2fd520dca58c654d2850ea9`);
disk round-trip `replay_json` `REPRODUCED`.

## 5. PR #286 — Auth Classification and Replay

PR #286 (`c39352bf`) is the auth-classification fix. Evidence that it works
on a real `require_auth` contract:

- **AuthMode reaches the Oracle** — `oracle.rs` exposes
  `Oracle::with_auth_mode(expected, auth_mode)`; the `AuthorizationBypass`
  rule is gated on `auth_mode != CorrectAuth`. `campaign.rs` wires
  `input.auth_mode` into the oracle for every case.
- **CorrectAuth success is not AuthorizationBypass** — see negative control
  below.
- **Multi-step replay reconstructs auth per step** — `replay.rs` rebuilds
  `auth_per_step` from `artifact.auth_mode` (one entry list per step, root =
  that step's call) and executes via `execute_sequence_auth`.
- **Lineage A/B on the identical multi-step `CorrectAuth` artifact**:

  | Lineage | Replay verdict |
  |---|---|
  | #286 (`c39352bf`) | `REPRODUCED` — auth entries preserved |
  | pre-#286 (`3f2a19f9`) | `MISMATCH` — expected `UnexpectedError`, actual `ExpectedError` (auth entries lost) |

- **Deterministic cross-run verification** — probe output byte-identical
  across processes; CLI campaign evidence byte-identical; artifact stream
  hash `d30bd3cee95f8e93…` identical.
- **Test suite** — `cargo test -p sdkt-fuzz` on a clean worktree at
  `c39352bf`: **57 passed, 0 failed**. #286-specific suites:
  `multistep_replay_auth` 1/1, `auth_oracle_matrix` 5/5,
  `auth_campaign_matrix` 4/4.

## 6. Negative Control

Unguarded external contract `increment` (WASM SHA-256
`36bc3b311c3780c847e5724d9dc0d5f160af22d476535a2881ea9155d2049dce`),
declared `auth_required = true` on an authorized (`CorrectAuth`) success:

| Lineage | Classification |
|---|---|
| #286 | `Pass` — no false bypass |
| pre-#286 | `Finding(AuthorizationBypass)` — false positive |

This demonstrates the specific oracle regression fixed by #286: before the
fix, an authorized success was reported as a bypass.

## 7. Findings

All pilot findings are **declaration mismatches**:

- `RETURN_MISMATCH` — a declared error expectation met a successful return.
- `UNEXPECTED_ERROR` — a declared success expectation met a host error.

They are evidence that the oracle, minimizer, artifact, and replay machinery
behave as designed. **They are not vulnerabilities**, and the audited example
contracts are not claimed to be defective.

## 8. Limitations

- No vulnerability / security-discovery claim is made or implied.
- No coverage or corpus metric exists; completeness is not claimed.
- Execution is offline in-memory `soroban-env-host` only; no network, no
  ledger, no transactions.
- CLI v2.7.0 lacks `--sequence`; sequence evidence uses the public library
  API (path-dependency probe, zero source modification).
- Auth evidence is single-signer source-account scope (`SourceAccount` vs
  `AddressV2`).
- No multi-signer or auth-tree semantics are exercised or claimed.
- The host does not expose an authorization trace; expectations are declared
  oracle outcomes only.
- The campaign minimizer can collapse a sequence to one step; the multi-step
  auth artifact was explicitly constructed via the public API.
- `stellar/soroban-examples` is not being security-audited.
- All findings are declaration mismatches.

## 9. Reproducibility

| Item | Value |
|---|---|
| External source | `stellar/soroban-examples` @ `03d42aa6b973dcf3a453a99d0c6a6e8d25a196e2` |
| Build | `stellar contract build` 28.1.0, `wasm32v1-none` |
| Pilot #1 seeds | `42` (RUN A, 50 cases/contract), `7` (RUN B, 50 cases), `42` (RUN C1, 40 cases, `sequence_length=3`, `sequence_every=2`) |
| Pilot #2 seeds | `42` (12-case campaigns), `7` (E2 multi-step artifact) |
| Clean verification worktree | `c39352bfcacf12912e1a6103848e6c0c978a0db6` |
| Canonical artifact hash (Pilot #1 sample) | `0ffdcd2395106c3c5a379cd6d482dbefc941efad95263f9fb57e3cf99c36c761` |
| Canonical artifact hash (Pilot #2 E2) | `a16041e410b4bef28d959f06ed827a20047ecffcc2a31a51be8c6c713e72e36a` |
| Campaign evidence SHA-256 (Pilot #1 increment) | `54a5c9b86d4c582d4f0cce7d257d6889afbfdd9a6165d5b95538e0d05137c918` |

Non-primary side-run artifacts were excluded from the deterministic evidence
set.

## 10. Safe Claim

> The sdkt-fuzz v2.7.0 pipeline (PR #285) and its auth-classification fix
> (PR #286) are externally validated on real Soroban contracts built from
> stellar/soroban-examples (HEAD 03d42aa6): ContractSpec parsing, typed
> generation and mutation, real in-memory host execution, oracle
> classification, minimization, canonical artifacts, and deterministic replay
> all reproduce byte-identically across processes. PR #286's auth path is
> externally validated on a contract that genuinely enforces require_auth:
> NoAuth and WrongAuth produce ContractError{Auth,6}, CorrectAuth succeeds
> with state carry, an authorized success is not classified as
> AuthorizationBypass, and multi-step replay preserves per-step auth entries
> (the pre-#286 lineage mismatches the same artifact). All findings are
> declaration mismatches — evidence that the engine works, not vulnerability
> discovery.
