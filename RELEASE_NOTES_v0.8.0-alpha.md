# Release Notes — sdkt v0.8.0-alpha

## Soroban Development Kit · Milestone 8 (Interactive Transaction Lifecycle)

### Scope

This release turns the read-only devkit into an interactive environment covering
the full Soroban transaction lifecycle: **simulate → build → manage keys →
submit & settle** — still without signing/broadcast-with-keys (planned as a
follow-up Milestone 9).

---

## ENG-07 — Transaction Simulation

- New `simulate` module in `sdkt-rpc`.
- `simulate_transaction` reuses `SorobanRpcClient::request` for transport
  (timeout + single transient retry already baked in — no HTTP duplication).
- Types: `SimulateResponse`, `SimulateOperationResult`, `SimulateCost`,
  `SimulateTransactionRequest` (all `Serialize`/`Deserialize`).
- `validate_envelope` pure guard against empty envelopes.
- CLI: `sdkt tx simulate --envelope <file|base64> [--format pretty|json]`.

## ENG-08 — Transaction Envelope Builder

- New `builder` module in `sdkt-xdr` (business types kept out of the CLI).
- `build_invoke_transaction` assembles an unsigned `TransactionEnvelope` using
  the existing `stellar_xdr` types only (no re-created XDR structs).
- Supports: source account, sequence number, fee, memo, timeout, and
  `InvokeHostFunction` operation with `ScVal` arguments.
- StrKey parsing via `stellar-strkey`; invalid checksum keys rejected early.
- CLI: `sdkt tx build --source <G...|identity> --sequence <N> --fee <N>
  --contract <C...> --function <fn> [--arg <b64>...] [--output <file.xdr>]`.
- `--source` accepts an identity name (ENG-09) and resolves to its public key.

## ENG-09 — Identity & Keystore Management

- New `identity` module in `sdkt-storage`.
- `IdentityStore` backed by `ed25519-dalek` (+ `stellar-strkey`, `rand`),
  stored in the OS-portable config dir via `directories` (`~/.config/sdkt/identities/`).
- Full lifecycle: `generate`, `import` (`S...` secret), `load`/`get` by name,
  `list`, `remove`, `set_default`/`get_default`.
- Storage hardening: key files written with **0600** permissions; secret keys
  rendered only through `stellar_strkey::Unredacted` (never via `Debug`/`Display`).
- CLI: `sdkt identity generate|import|list|show|delete|default`.

## ENG-10 — Transaction Submission Engine

- New `submission` module in `sdkt-rpc`. All transport via `SorobanRpcClient`
  (no duplicated HTTP logic).
- Functions:
  - `send_transaction` → `sendTransaction`
  - `get_transaction_status` → `getTransaction`
  - `poll_transaction` → loop until SUCCESS/FAILED/NOT_FOUND or timeout
  - `submit_and_wait` → submit, optionally poll (ENG-07/08/09 ready)
- Types: `SendTransactionRequest`, `SendTransactionResponse`,
  `TransactionStatusResponse`, `TransactionStatus` (Pending/Success/Failed/NotFound),
  `SubmissionResult`, `PollConfig`.
- Polling: configurable timeout & interval + lightweight exponential backoff;
  stops on SUCCESS or FAILED; clear timeout error.
- Retry policy limited to transient RPC/network errors (handled in the client).
- CLI: `sdkt tx submit --envelope <file|base64> [--wait] [--timeout N]
  [--interval N] [--format pretty|json]`. Without `--wait`, submits and prints
  the hash (status Pending).

---

## Reuse & Architecture

- All HTTP/JSON-RPC transport, timeouts and transient retry live in
  `SorobanRpcClient` (ENG-02 era) — ENG-07/10 never duplicate it.
- ENG-08 reuses `stellar_xdr` types and `stellar-strkey`. ENG-09 reuses
  `stellar-strkey` for StrKey codec. ENG-10's `SubmissionResult` lines up with
  ENG-07 response shapes and ENG-08's envelope output (`--output <file>` feeds
  directly into `tx submit --envelope <file>`).
- No signing, multisig, or offline-signing implemented (allocated to Milestone 9).

## Test Summary

- Total workspace tests: **106**
  - Unit + integration across `sdkt-core`, `sdkt-xdr`, `sdkt-wasm`,
    `sdkt-rpc`, `sdkt-storage`, `sdkt-cli`.
  - New: simulate (6), envelope builder (6), identity (4),
    submission (5) units; integration for build / simulate / submit / identity.
- All quality gates green: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace`.

## Breaking Changes

- None to previously released public API. New modules (`simulate`, `builder`,
  `identity`, `submission`) are additive. `sdkt-rpc`/`sdkt-core` editions remain
  2021; the E0670 "async in Rust 2015" reports from the metadata linter are
  known context-tool false positives on edition-2021 crates (verified via real
  `cargo check`).

## Known Limitations

- **No signing** — built/simulated/submitted envelopes must be signed
  out-of-band (planned: Milestone 9 signing engine + `SigningKey` integration).
- `submit_and_wait` requires `whitelist`-free RPC access; NOT_FOUND falls through
  to the Pending/timeout path (a NOT_FOUND result maps to timeout error).
  A dedicated NOT_FOUND short-circuit is a follow-up.
- CLI integration tests for `submit` exercise the error path (no live node
  required); happy-path broadcast tests need a funded testnet account.
- Identity key files use 0600 + redacted debug output but are **not**
  passphrase-encrypted on disk (roadmap: optional AES via a password prompt).

## Commits

- `feat(rpc): add transaction simulation support (ENG-07)`
- `feat(core): add transaction envelope builder (ENG-08)`
- `feat(storage): add identity and keystore management (ENG-09)`
- `feat(rpc): add transaction submission engine (ENG-10)`