# Milestone 5 Architecture Plan: Expanded Developer Toolkit

## Goal
Expand Soroban DevKit (`sdkt`) from a smart contract inspector into a comprehensive developer toolkit for the Soroban ecosystem by introducing network state introspection tools. 

## Proposed Commands

### 1. Transaction Inspection
View detailed network transaction status and operations.
**Command**: `sdkt tx inspect TRANSACTION_HASH`
**Expected Output**:
- Transaction Hash
- Network Status (Success, Failed, Pending)
- Ledger sequence inclusion
- Fee charged (stroops)
- Operations count
- Emitted Soroban Events (summary/count)

### 2. Event Explorer
Filter and view emitted contract events for auditing and tracking.
**Command**: `sdkt events CONTRACT_ID`
**Expected Output**:
- Topic list (base64 or decoded ScVal if feasible)
- Values (decoded ScVal JSON)
- Ledger sequence range / block time

### 3. Account Inspection
Quick diagnostic of Stellar network accounts and Soroban associations.
**Command**: `sdkt account ADDRESS`
**Expected Output**:
- XLM Balance
- Sequence number
- Other associated assets
- Associated contracts / signer status

## Architecture Impact & Boundaries

### `sdkt-rpc` Crate
- Needs new RPC JSON-RPC endpoints added to `SorobanRpcClient` (e.g. `getTransaction`, `getEvents`, or bridging to Horizon/Stellar RPC where necessary).
- Keep standard JSON parsing bounded behind standard library struct representations inside `sdkt-rpc/src/tx.rs`, `events.rs` and `account.rs`.

### `sdkt-xdr` Crate
- May require expanding decoding capabilities beyond `ScVal` and `LedgerEntry` to support `TransactionEnvelope`, `TransactionResult`, and `ContractEvent`.

### `sdkt-cli` Crate
- Addition of subcommands to parser for `tx`, `events`, and `account`.
- Minimal formatting logic in CLI, handing off output mapping to traits if formatting grows complex.

## Dependency Changes
- Minimal. We continue to rely on `reqwest` and `stellar-xdr`.
- No new heavy dependencies will be introduced. Use existing JSON serialization.

## Testing Strategy

All new commands will require integration tests simulating standard inputs.

`tests/` structure:
- `tests/tx_integration_test.rs` (Tests `tx inspect`)
- `tests/events_integration_test.rs` (Tests `events` list)
- `tests/account_integration_test.rs` (Tests `account` lookup)

Unit tests inside `sdkt-rpc` will continue to use stubbed JSON-RPC responses for deterministic tests.

## Status

- [x] Transaction Inspection
- [x] Event Explorer
- [x] Account Inspection
