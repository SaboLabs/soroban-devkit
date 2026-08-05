# Soroban DevKit (`sdkt`)

Soroban DevKit is a unified toolkit for Stellar/Soroban development, providing utilities for inspecting contracts, decoding XDR, analyzing storage TTLs, and introspecting network state (transactions, events, and accounts).

## Project Overview
`sdkt` serves as the swiss army knife for Soroban developers, unifying otherwise disconnected tools into a single robust CLI. It communicates natively via Soroban RPC and handles XDR decoding transparently.

## Installation

Ensure you have Rust installed (Edition 2021 required), then build from source:

```bash
git clone https://github.com/yourusername/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli
```

## Quick Start
Check a smart contract's raw storage limits:
```bash
sdkt inspect CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK
```

## CLI Commands

### 1. XDR Decoding
Decode a payload into pretty output:
```bash
sdkt decode "AAAAAwAAAAE=" --type ScVal --format pretty
```

### 2. Contract Inspection
Inspect contract details (returns WASM hash and storage entries count):
```bash
sdkt inspect CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK
```

### 3. Storage TTL Check
Check remaining storage TTL ledgers for a given contract:
```bash
sdkt storage check CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK --format json
```

### 4. Transaction Inspection
View detailed network transaction status:
```bash
sdkt tx inspect <TRANSACTION_HASH>
```

### 5. Event Explorer
Filter and view emitted contract events for auditing:
```bash
sdkt events <CONTRACT_ID>
```

### 6. Account Inspection
Quick diagnostic of a Stellar network account:
```bash
sdkt account <ADDRESS>
```

## JSON Output Example
Most commands support `--format json` for integration into CI or scripts:
```json
{
  "address": "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
  "sequence": null,
  "balances": [],
  "signers": []
}
```

## Workspace Structure
The project is split into independent workspace crates:
- `sdkt-cli`: The command line interface.
- `sdkt-core`: Common configurations and output formats.
- `sdkt-rpc`: Soroban RPC integration layer.
- `sdkt-storage`: Storage analysis routines.
- `sdkt-xdr`: XDR encoding/decoding utilities based on stellar-xdr.

## Development
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Testing
We utilize `assert_cmd` for CLI integration tests stored under `crates/sdkt-cli/tests/`. Run them via `cargo test --workspace`.

## Roadmap
- Improve robust parsing of raw ContractEvent XDR values.
- Integrate full Horizon endpoints for richer Account metadata.
- Implement storage estimate commands.
