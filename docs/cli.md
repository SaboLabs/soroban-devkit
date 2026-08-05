# CLI Command Flow

The `sdkt-cli` crate uses `clap` (derive API) for command routing.

## Current Command Structure

```text
sdkt
├── decode [payload]
│   ├── --type <type>
│   ├── --format <json|pretty>
│   └── --file <path>
│
├── storage
│   ├── check <contract-id>
│   │   └── --format <json|pretty>
│   └── estimate <wasm-path>
│
└── inspect <contract-id>
    └── --format <json|pretty>
```

## Planned Extensibility (Milestone 4+)

To ensure the CLI remains clean, new domains are grouped as subcommands:

```text
sdkt
├── account
│   ├── balance <address>
│   └── history <address>
│
├── tx
│   ├── simulate <xdr>
│   └── submit <xdr>
│
└── events
    └── listen <contract-id>
```

## Error Handling
1. All subcommands return `Result<(), Box<dyn std::error::Error>>`.
2. Library-level errors (`RpcError`, `DecodeError`) are bubbled up to the CLI.
3. The CLI uses `eprintln!` to print human-readable errors and calls `std::process::exit(1)` on fatal errors to ensure proper bash piping behavior.