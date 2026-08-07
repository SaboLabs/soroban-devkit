# CLI Command Reference

The `sdkt-cli` crate uses `clap` (derive API) for command routing. Every command returns `Result<(), Box<dyn std::error::Error>>`; library errors (`RpcError`, `DecodeError`, `WasmError`) bubble up and are printed via `eprintln!` with a non-zero exit.

## Command Tree

```text
sdkt
├── decode <xdr>
│   ├── --type <ScVal|TransactionEnvelope|ContractEvent>
│   ├── --format <json|pretty>
│   └── --file <path>
│
├── inspect <contract-id>
│   ├── --format <json|pretty>
│   └── --abi <wasm>            (ABI-aware storage decode)
│
├── storage
│   ├── check <contract-id>   [--abi <wasm>] [--format]
│   ├── analyze <contract-id> [--format]
│   └── estimate <wasm-path>  [--format]
│
├── tx
│   ├── inspect <hash>        [--format]
│   ├── validate <xdr>        [--format] (offline parse + structural checks)
│   ├── simulate <xdr>        [--format] (RPC; surfaces restore preambles, costs, state changes)
│   ├── sign                  [--input <xdr|file>] [--output <file>] [--identity <name>] [--network <testnet|mainnet|futurenet|custom:<p>>] [--format] (offline ED25519 signing)
│   ├── submit <xdr>          [--wait] [--timeout <s>] [--interval <s>] [--format] (RPC)
│   └── build                 [--source --sequence --contract --function --arg* --output]
│
├── events <contract-id>
│   ├── --format <json|pretty>
│   └── --abi <wasm>          (ABI-aware decode)
│
├── account <address>         [--format]
│
├── fee
│   └── estimate              (manual value entry, type-prefixed)
│
├── wasm
│   ├── inspect <file.wasm>  Offline inspection of a local WASM file (sections, exports, spec)
│   ├── metadata --contract <contract>  [--network testnet] [--refresh] [--format]
│   └── cache                 (info | remove | clear)
│
├── verify --contract <contract-id>
│   ├── --wasm <file.wasm>    (local artifact to compare; offline hashed)
│   ├── --network <testnet>   (RPC network)
│   └── --format <json|pretty>
│
├── health --contract <contract-id>
│   ├── --wasm <file.wasm>    (optional local artifact to verify against)
│   ├── --network <testnet>   (RPC network / report label)
│   └── --format <json|pretty>
│
├── diff
│   ├── --old-wasm <A>
│   ├── --new-wasm <B>
│   ├── --format <json|pretty>
│   └── --upgrade-safety      (emit UpgradeVerdict)
│
├── audit <path.rs>
│   ├── --format <json|pretty>
│   ├── --disable <RULE_ID>   (repeatable)
│   └── --rules <PATH>        (repeatable; external rule paths)
├── identity
│   ├── generate <name>
│   ├── import <name> <secret>
│   ├── list
│   ├── show <name>
│   ├── delete <name>
│   └── default <name>
│
├── init <name>              [--minimal] [--force] [--format]
│
├── build                     Compile Rust contracts in the workspace into WASM artifacts
│
├── project
│   └── deploy                Deploy all contracts defined in the workspace (.sdkt.toml),
│                             applying topological dependency sorting
│
└── deploy
    ├── --wasm <file>
    ├── --salt <salt>
    ├── --format <json|pretty>
    ├── --deny-breaking        (abort if not backwards-compatible)
    └── --old-wasm <deployed>  (baseline, required by --deny-breaking)
```

## Notes

- `--format json` is supported on all read-style commands and on `diff`, `audit`, `deploy`, `init` for scripting / CI.
- `diff --upgrade-safety` and `deploy --deny-breaking` implement the Milestone 14 Upgrade Safety Guard (see `ROADMAP.md`).
- `audit` implements the Milestone 13 static-analysis rules (AUTH-001/002/003, MOVE-001).

## Error Handling

1. All subcommands return `Result<(), Box<dyn std::error::Error>>`.
2. Library-level errors (`RpcError`, `DecodeError`, `WasmError`) are bubbled up to the CLI.
3. The CLI uses `eprintln!` to print human-readable errors and exits non-zero on fatal errors to ensure correct bash piping behavior.
