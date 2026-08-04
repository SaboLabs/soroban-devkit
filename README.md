# Soroban DevKit (`sdkt`)

Soroban DevKit is a unified toolkit for Stellar/Soroban development, providing utilities for inspecting contracts, decoding XDR, and analyzing storage TTLs.

## Installation

Ensure you have Rust installed, then build from source:

```bash
git clone https://github.com/yourusername/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli
```

## Features

- **XDR Decoding**: Decode base64 XDR payloads into JSON or pretty format.
- **Contract Inspection**: Query a contract's ABI and raw storage properties.
- **Storage Analysis**: Check remaining TTLs for a contract's storage keys.

## Usage Examples

### XDR Decoding

Decode a payload into pretty output:

```bash
sdkt decode "AAAAAwAAAAE=" --type ScVal --format pretty
```

### Contract Inspection

Inspect contract details (returns WASM hash and storage entries count):

```bash
sdkt inspect CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK
```

### Storage TTL Check

Check remaining storage TTL ledgers for a given contract:

```bash
sdkt storage check CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK --format json
```

## Architecture

The project is split into workspace crates:
- `sdkt-cli`: The command line interface.
- `sdkt-core`: Common configurations and output formats.
- `sdkt-rpc`: Soroban RPC integration layer.
- `sdkt-storage`: Storage analysis routines.
- `sdkt-xdr`: XDR encoding/decoding utilities based on stellar-xdr.

