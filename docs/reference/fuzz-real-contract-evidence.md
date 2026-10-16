# Real-Contract Fuzz Evidence

## 1. Purpose

This page is **evidence that the `sdkt-fuzz` v2.7.0 pipeline actually runs on
external Soroban WASM** — not a security audit, not a vulnerability-discovery
proof, and not a completeness claim.

Three independent pilots were executed offline against external Soroban
contracts: two from the official
[`stellar/soroban-examples`](https://github.com/stellar/soroban-examples)
repository (HEAD `03d42aa6b973dcf3a453a99d0c6a6e8d25a196e2`) and one from the
production-like [`SUSU-LABS/susu-contracts`](https://github.com/SUSU-LABS/susu-contracts)
rotating-savings protocol (commit
`4e3f745530728062303c445430cfad9d17e81227`). Every claim below is backed by a
reproducible artifact: a WASM SHA-256, a canonical artifact hash, a replay
verdict, or a byte-identical rerun.

## 2. Environment

| Component | Version / value |
|---|---|
| `sdkt-fuzz` | v2.7.0 |
| Pipeline PR | #285 — deterministic Soroban fuzzing and replay |
| Auth fix PR | #286 — correct auth classification and replay |
| Host engine | `soroban-env-host` 28.0.2 (in-memory, offline) |
| XDR | `stellar-xdr` 28.0.0 |
| Build toolchain | `stellar contract build` 28.1.0 (`wasm32v1-none`) |
| External source | `stellar/soroban-examples` @ `03d42aa6`; `SUSU-LABS/susu-contracts` @ `4e3f7455` |
| Execution model | offline, in-memory Host only — no network, no ledger, no transactions |

> Note on version compatibility: the SUSU group contract is built with
> `soroban-sdk` 27.0.6 while the engine pins `soroban-env-host` 28.0.2 /
> protocol 28. The WASM loads and executes correctly in this host; the version
> gap is recorded as a compatibility observation, not described as a defect.

## 3. Pilot #1 — External Contracts (no `require_auth`)

Four official example contracts were compiled with the Stellar CLI and fuzzed
through the public CLI and library API.

| Target |WASM SHA-256 |
|---|---|
| `increment` | `36bc3b311c3780c847e5724d9dc0d5f160af22d4765352a2881ea9155d2049dce` |
| `custom_types` | `2f6f5657cb8ec38227ba636de221aed75ac31ac6db9614b710f73ae5686ebd40` |
| `events` | `33084e80e278a22703f6bd4f70cc3e70d4bd3718468ab5401202c36f36a0ef7c` |
| `errors` | `c958ec5683d4e98d5e3edfcfb3e07a2d33978579197bceb0394bf3d8e0ef99a8` |

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
  `ContractError{Contract,1}` → `ExpectedError` (exactly at `MAX + 1`).
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

## 5. Pilot #3 — Real Auth + State Machine (`SUSU-LABS/susu-contracts`)

Target: `SUSU-LABS/susu-contracts` (commit
`4e3f745530728062303c445430cfad9d17e81227`), contract `contracts/group`
(crate `susu-group`) — the rotating-savings ("susu") protocol's group
contract: one instance per group, financial authority for the pool. 17
exported functions; `member.require_auth()` guards `join` and `contribute`;
`start()` is permissionless by design.

WASM SHA-256:

```
e5048723f3383c13ae57a3f3a5d78d070f1525bbc2ae4476df9ad16d8b51bcf2
```

(22530 bytes, `stellar contract build` 28.1.0, `soroban-sdk` 27.0.6.)

### Auth matrix

Seeded instance baseline (`Status=Open`, `MemberCount=0`), deterministic
addresses (engine source account + a foreign address):

| Mode | `join` | `contribute` |
|---|---|---|
| `NoAuth` | `ContractError{Auth,6}` | `ContractError{Auth,6}` |
| `CorrectAuth` | `ContractError{Storage,5}` | `ContractError{Contract,9}` |
| `WrongAuth` | `ContractError{Auth,6}` | `ContractError{Auth,6}` |

Oracle classification (`Oracle::with_auth_mode`):

| Mode | declared `Error(Auth,6)` |
|---|---|
| `NoAuth` / `WrongAuth` | `ExpectedError` |
| `CorrectAuth` | `Finding(UnexpectedError)` — auth **passed**, execution proceeded past `require_auth` |

Reading the `CorrectAuth` row (D5): `join` → `ContractError{Storage,5}`
(`ScErrorCode 5 = ExceededLimit`) means authentication succeeded and execution
then reached a persistent `Member(source)` read that was not present in that
step's footprint/baseline — this is footprint/state setup behavior, **not** an
auth failure. `contribute` → `ContractError{Contract,9}` is the contract's own
`NotActive` business error. NoAuth/WrongAuth produce `ContractError{Auth,6}` as expected. This is behavioral observation, not a protocol issue.

### State-machine boundary (6 distinct `GroupError` codes, real execution)

Single calls under `CorrectAuth` against seeded instance + persistent
baselines; each produced a distinct contract error code:

| Seeded state | Call | Result | `GroupError` |
|---|---|---|---|
| `Member(source)` present | `join(source)` | `ContractError{Contract,6}` | `AlreadyMember` |
| `Status=Active` | `join(source)` | `ContractError{Contract,5}` | `NotOpen` |
| `Status=Draft` | `contribute(...)` | `ContractError{Contract,9}` | `NotActive` |
| `amount != contribution_amount` | `contribute(...)` | `ContractError{Contract,12}` | `WrongAmount` |
| `round != current_round` | `contribute(...)` | `ContractError{Contract,11}` | `WrongRound` |
| `MemberCount < capacity` | `start()` | `ContractError{Contract,8}` | `CapacityNotReached` |

### State propagation (decisive)

`execute_sequence_auth` carries instance storage **and** persistent data
entries between steps. An earlier report draft incorrectly attributed a sequence failure to an engine
state-propagation limitation. That attribution was wrong: the failing probe
sequences passed an **empty baseline** (probe design), while the engine does
propagate state between steps, as the probe below shows.

Decisive probe (`probe4`), instance baseline
`Status=Open, MemberCount=2 (=capacity), CurrentRound=0`:

| Step | Call | Result |
|---|---|---|
| 0 | `get_status()` | `Returned(Open)` |
| 1 | `start()` | `Void` (permissionless; capacity reached — legal) |
| 2 | `get_status()` | `Returned(Active)` — step 1's write visible |
| 3 | `get_current_round()` | `Returned(U32(1))` — step 1's write visible |
| 4 | `get_group()` | `Returned(full GroupState map)` |

Final carried instance state: `Status=Active`, `CurrentRound=1`,
`MemberCount=2`, `RoundPhase=WaitingForContributions`, `Config` intact.
`probe4` run1/run2 output is byte-identical across processes.

Soroban context: contract-data access is footprint-bounded by protocol
semantics (an invocation reads/writes the `CONTRACT_DATA` entries declared in
its footprint). The engine builds each step's footprint from the carried state
and baseline entries; a read of a key absent from that footprint surfaces as
pStorage,5` (`ExceededLimit`). That is setup behavior, not a state-loss bug —
the carry proof above shows writes propagating between steps.

### Token dependency

`contribute` and `execute_payout` call `token::Client::transfer` against
`config.token`. No token contract exists in the local host baseline, so the
full happy-path `contribute`/`payout` is not reachable in this probe:

> Token dependency prevents full happy-path execution in this probe.

This is an external-contract dependency, not an engine limitation. Everything
up to the token transfer — auth, state-machine validation, storage writes —
is exercised and observed.

### Campaign

Seed `42`, 12 cases, `mutations_per_case=2`, per auth mode
(`no_auth` / `correct_auth` / `wrong_auth`): 12 executed, 10 passed, 0
expected-errors, 2 findings per mode, 2 artifacts per mode, 6 artifacts
total; all `UNEXPECTED_ERROR` (declared success vs host error).

### Minimization (both samples)

| Finding | Complexity | `preserved` | Reductions |
|---|---|---|---|
| `join` case0000 | `14 -> 14` | `true` | 0 — single `Address` argument already minimal |
| `contribute` case0006 | `49 -> 18` | `true` | 1 — `step 0 arg 2: u32(1339550796) -> u32(0)` |

The `14 -> 14` case is genuinely minimal (nothing to reduce); `preserved=true`
is verified by re-execution producing the same reason code, not vacuously.

## 6. PR #286 — Auth Classification and Replay

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
  hash `d30bd3cee
