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
│   ├── check <contract-id>   [--abi <wasm>] [--abi-contract <id>] [--format]
│   ├── analyze <contract-id> [--abi <wasm>] [--abi-contract <id>] [--format]
│   ├── estimate <wasm-path>  [--format] (NOT YET IMPLEMENTED — placeholder only)
│   ├── read --contract <contract-id> --key-xdr <BASE64_XDR> [--abi <wasm>] [--format]
│   └── extend --contract <contract-id> --ledgers <N> [--key <xdr>]... [--identity <name>] [--format]
│
│   `read` fetches a single ledger entry by its complete `LedgerKey` (base64 XDR).
│   The instance key is NOT included automatically — supply the full key via --key-xdr.
│   ABI formatting is optional and applies only to the returned value.
│
│   `extend` submits an `ExtendFootprintTtl` transaction. The contract instance
│   key is always included in the read-only footprint. Additional `--key` values
│   (base64 XDR or hex XDR) are merged and de-duplicated. This does NOT discover
│   all contract storage and does NOT restore archived entries.
│
│   `--abi <wasm>` supplies the ABI from a local WASM; `--abi-contract <id>` fetches
│   the deployed contract's on-chain WASM and uses it as the ABI source
│   for storage decoding. The two flags are mutually exclusive.
│
├── invoke <CONTRACT_ID> <FUNCTION>
│   ├── --args <TYPE:VALUE>...    (same typed-args as `call` / `tx build`;
│   │                            strict: unknown types are rejected)
│   ├── --identity <name>        (signs and pays; default: "default")
 main
│   ├── --format <json|pretty>
│   └── --network-profile <NAME> / --rpc-url <URL> / --network-passphrase <P>
│
│   State-changing end-to-end flow in one command:
│     fetch account sequence → simulate → build final envelope (authoritative
│     footprint + fees + auth entries from simulation) → sign with the local
 main
│   Result decoding is limited to the transaction-level `TransactionResult`
│   XDR (no ABI-aware result decode yet). Inherits the mainnet safety guard
│   (see below). Live Testnet smoke test is documented but NOT exercised in CI.
│
├── tx
│   ├── inspect <hash>        [--format]
│   ├── validate <xdr>        [--format] (offline parse + structural checks)
│   ├── simulate <xdr>        [--format] [--abi <wasm>] [--abi-contract <id>] (RPC; surfaces restore preambles, costs, state changes; ABI-aware result decoding. `--abi-contract` fetches the deployed contract's on-chain WASM for decoding)
│   ├── sign                  [--input <xdr|file>] [--output <file>] [--identity <name>] [--network <testnet|mainnet|futurenet|custom:<p>>] [--format] (offline ED25519 signing)
│   ├── submit <xdr>          [--wait] [--timeout <s>] [--interval <s>] [--format] (RPC)
│   └── build                 [--source --sequence --contract --function --fee* --arg* --output]
│                             Offline by default: the envelope carries only the
│                             base inclusion fee (100 stroops) and prints a
│                             warning, because a Soroban submission also needs
│                             the resource fee.
│                             Pass the network flags on `tx` (e.g.
│                             `sdkt tx --network-profile testnet build ...`) to
│                             simulate the invocation and adopt the reported
│                             `minResourceFee` and footprint, exactly as
│                             `invoke` does — the resulting envelope submits
│                             without re-simulating.
│                             `--fee <STROOPS>` overrides both and costs no
│                             network round trip.
│
├── events <contract-id>
│   ├── --format <json|pretty>
│   ├── --abi <wasm>          (ABI-aware decode from a local WASM)
│   └── --abi-contract <id>   (ABI-aware decode using the deployed contract's
│                               on-chain WASM, fetched via the inspection path;
│                               no local artifact needed. Mutually exclusive
│                               with --abi.)
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
│   ├── --format <json|pretty>
│   └── --upgrade-safety      (compare the live deployed contract's interface
│                               against --wasm candidate; emits UpgradeVerdict.
│                               Requires --wasm. Read-only; inherits mainnet-safety
│                               guard.)
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
├── audit [path.rs]
│   ├── --list-rules          (list available audit rules and exit)
│   ├── --format <json|pretty>
│   ├── --disable <RULE_ID>   (repeatable)
│   ├── --rules <PATH>        (repeatable; external rule paths)
│   └── --no-plugins          (skip loading installed plugins)
├── identity
│   ├── generate <name>
│   ├── import <name> <secret>
│   ├── list
│   ├── show <name>
│   ├── delete <name>
│   ├── default <name>
│   └── fund <name>           [--network-profile <NAME>] [--format pretty|json]
│
├── init <name>              [--minimal] [--force] [--format]

├── network
│   ├── add <name>           [--rpc-url <URL>] [--passphrase <PASS>] [--friendbot <URL>] [--description <TEXT>]
│   ├── list
│   ├── show <name>          [--format json|pretty]
│   └── remove <name>

├── build                     Compile Rust contracts in the workspace into WASM artifacts

├── lock                      Generate or inspect `sdkt.lock`
│   ├── generate              Write `sdkt.lock` from current build artifacts (run `sdkt build` first)
│   ├── verify                Verify `sdkt.lock` against on-disk contract artifacts AND package
│   │                         dependencies (lock matches manifest, git commits, path existence).
│   │                         Advisory and non-fatal: prints `✓ lock file verified` /
│   │                         `✓ package dependencies verified` or lists every drift.
│   └── show                  Print `sdkt.lock` contents

├── package                   Validate local package manifests
│   ├── validate              Offline-validate `[package]` metadata + local `[dependencies]`
│   │                         path graph (no network/registry; git/* sources rejected)
│   ├── fetch                 Fetch deps into `.sdkt-cache`: local path passthrough,
│   │                         git clone/checkout. `--force` updates. Never builds.
│   ├── update                Synchronize deps: refresh git deps to latest available
│   │                         commit and rewrite `sdkt.lock`. `rev` pinned; `tag`/`branch`
│   │                         update on drift. `--check` reports; `--dry-run` previews.
│   ├── pack                  Bundle the resolved project into a portable offline artifact
│   │                         manifest + lock + cached git checkouts. `--out`, `--format`.
│   └── publish               Validate publish readiness (`--dry-run` only, read-only);
│                             detects missing cache, lock drift, integrity mismatch.

### Synchronizing dependencies

`sdkt package update` closes the package loop: `validate → fetch → update → verify`.

It resolves each git dependency's **currently available** commit via `git ls-remote`
against its declared URL (no checkout, no clone in `--check`/`--dry-run`), then:

- **`rev`** — immutable and already pinned; never updated. Reported as `pinned (rev)`.
- **`tag`** — resolves the tag's current commit; if it differs from the lock, the
  cache is refreshed and the lock is rewritten.
- **`branch`** — fetches the latest branch head, updates the cache, and rewrites the
  lock.
- **local `path`** — no remote; reported `unchanged`.

Flags:

- `--check` — report available updates only; **does not** fetch or rewrite the lock;
  exits 0 (non-zero is reserved for hard errors like a missing lock or invalid manifest).
- `--dry-run` — compute everything and preview what would change; the cache and lock
  are left untouched.
- `--format pretty|json` — `json` emits `{"checked","updated","unchanged","changes":[...]}`.

On success the lock records the new `commit_sha`, `resolved_reference`, `cache_location`,
and `integrity` for git deps — contract entries, artifact hashes, and deploy order are
preserved. Clear errors are produced for: missing cache, git unavailable, invalid
manifest, missing lock, detached branch, unknown reference, and network failure.

### Version-constrained dependencies

A git dependency may declare an optional semver `version` constraint instead of a
fixed `tag` / `branch` / `rev`:

```toml
[dependencies.math]
git = "https://github.com/org/math"
version = ">=1.0, <2"
```

When `version` is set **without** an explicit ref, `sdkt package fetch` / `sdkt package
update` resolve the **highest remote tag that satisfies the constraint** (via
`git ls-remote --tags`; offline for local remotes) and materialize / lock that exact
tag. `--check` reports `constraint unsatisfied` when no tag matches. An explicit
`tag` / `branch` / `rev` always takes precedence and the constraint is ignored
(mirroring how `rev` / `path` deps bypass version resolution). The lock records the
resolved `version` for audit. This reuses the existing fetch / cache / lock
infrastructure; the only new logic is a single pure `VersionResolver`.

### Offline packaging & publish readiness

`sdkt package pack` bundles the **fully resolved** project into a portable,
network-free artifact so it can be reconstructed and rebuilt on another machine
without contacting any remote:

- the manifest (`.sdkt.toml`),
- the lockfile (`sdkt.lock`),
- the cached git dependency checkouts under `.sdkt-cache/git/<cache_key>`.

Each artifact carries a `package.json` descriptor (`PackageBundle`) recording the
package `name`/`version`, the chosen `format`, the `sdkt.lock` sha256, and a
per-dependency entry (`source`, `git_url`, `commit_sha`, `integrity`,
`cache_key`, `version`) so the bundle can be verified offline.

Flags:

- `--out <DIR>` — output directory (default `./dist`).
- `--format tar.zst|dir` — `tar.zst` writes a compressed tarball
  `<out>/<name>-<version>.tar.zst`; `dir` writes a directory tree
  `<out>/<name>-<version>/`. Any other value is rejected with a clear error.

`sdkt package publish --dry-run` validates **publish readiness** using the
existing manifest/lock/cache/integrity infrastructure. It detects missing cache,
lock drift, integrity mismatch, commit mismatch, reference change, and invalid
package state — all read-only, no network, nothing is published. `--broadcast` is
explicitly opt-in and is rejected because no registry source is defined; the
workflow remains fully offline.

Round-trip: a bundle can be reconstructed (`sdkt_core::package::unpack`) and the
reconstructed tree verified to reproduce the original `sdkt.lock` sha256 and
per-git-dependency integrity exactly (`verify_bundle_equivalence`) — no hashing
or git logic is duplicated; the same `compute_dependency_integrity` /
`git_cache_key` primitives are reused.

├── project
│   └── deploy                Deploy all contracts defined in the workspace (.sdkt.toml),
│                             applying topological dependency sorting
│
│   Contracts declare dependencies via `depends_on` (canonical) or the
│   legacy `deploy_after` field in `[contracts.<alias>]`; both are merged. Build,
│   deploy, and `sdkt lock generate` share one resolver, so order is deterministic.
│   Invalid graphs (unknown/self/duplicate dependency, cycle, duplicate name)
│   fail fast with a clear error.
│
├── call
│   ├── <CONTRACT_ID>
│   ├── <FUNCTION>
│   ├── --args <TYPE:VALUE>...    (e.g. u32:100, address:G..., string:hello)
│   ├── --abi <wasm>          (ABI-aware result decoding from a local WASM;
│   │                            without it the raw base64 XDR result is shown)
│   ├── --abi-contract <id>   (ABI-aware result decoding using the deployed
│   │                            contract's on-chain WASM, fetched via the
│   │                            inspection path; no local artifact needed.
│   │                            Mutually exclusive with --abi.)
│   ├── --format <json|pretty>
│   └── --network-profile <NAME>
└── deploy
    ├── --wasm <file>
    ├── --salt <salt>          (optional; auto-generated if omitted)
    ├── --format <json|pretty>
    ├── --deny-breaking        (abort if not backwards-compatible)
    └── --old-wasm <deployed>  (baseline, required by --deny-breaking)
```

## Network Profiles

Every RPC command (`inspect`, `verify`, `health`, `storage`, `events`, `account`,
`tx`, `fee`, `wasm`, `deploy`, `project deploy`) accepts the same three optional
flags for selecting / overriding the network endpoint:

| Flag | Meaning |
|------|---------|
| `--network-profile <NAME>` | Use a saved profile (see `sdkt network`). Loads its RPC URL + passphrase. |
| `--rpc-url <URL>` | Explicit RPC endpoint; overrides the profile and `.sdkt.toml`. |
| `--network-passphrase <PASSPHRASE>` | Explicit network passphrase; overrides the profile and `.sdkt.toml`. |

**Resolution precedence (highest wins):**

```
explicit --rpc-url / --network-passphrase
        > --network-profile <NAME>
                > .sdkt.toml [network]
                        > NetworkConfig::default()   (testnet)
```

`tx sign` is excluded — it is offline signing and takes only `--network` for the
signature hash. Commands invoked without these flags behave exactly as before.

## Shell completions

`sdkt` can emit shell completion scripts for the major shells. This is the
fastest way to discover subcommands and flags.

```bash
sdkt completions bash       # bash
sdkt completions zsh        # zsh
sdkt completions fish       # fish
sdkt completions powershell # powershell
sdkt completions elvish     # elvish
```

Pipe the output to a file your shell reads at startup (see the README
"Shell completions" section for per-shell install paths). Tab-completion then
covers commands, subcommands, and flag names.

## Doctor

`sdkt doctor` runs baseline diagnostic checks on the sdkt environment and the
current project. It is fully offline and safe to run anywhere.

```bash
sdkt doctor              # human-readable output
sdkt doctor --json      # machine-readable output
```

Checks included in the core:

| Check | ID | Meaning |
|-------|----|---------|
| sdkt runtime | `sdkt-runtime` | confirms the binary is running and reports its version |
| Rust toolchain | `rust-toolchain` | verifies `cargo` and `rustc` are on `PATH` |
| WASM build target | `wasm-target` | verifies a Soroban WASM target is installed via `rustup` |
| Project config | `project-config` | validates `.sdkt.toml` and the `[contracts]` dependency graph when inside a project |

Exit codes:

- `0` — all checks passed, or only warnings (non-fatal)
- `1` — at least one check failed (e.g. invalid config, missing WASM target)

Warnings are never fatal. The JSON output contains no secret material.

## Encode

`sdkt encode` converts a typed value to its base64 XDR representation — the
write-direction counterpart to `sdkt decode`. It is fully offline and useful
for building test fixtures, debugging, and CLI round-trips.

```bash
sdkt encode u32:100
# AAAAAwAAAGQ=

# Verify by decoding back
sdkt encode string:hello | xargs sdkt decode --type ScVal
# {"string": "hello"}

sdkt encode symbol:USD | xargs sdkt decode --type ScVal
# {"symbol": "USD"}

sdkt encode u128:340282366920938463463374607431768211455 | xargs sdkt decode --type ScVal
# {"u128": "340282366920938463463374607431768211455"}

sdkt encode i128:-1000 | xargs sdkt decode --type ScVal
# {"i128": "-1000"}

sdkt encode bytes:000aFF | xargs sdkt decode --type ScVal
# {"bytes": "000aff"}
```

### Supported types (core subset)

`u32`, `i32`, `u64`, `i64`, `u128`, `i128`, `bool`, `string`,
`symbol` (up to 32 bytes), `bytes`, `address` (Stellar `G...` strkey).

Exactly one value is encoded per invocation; the `TYPE:VALUE` syntax matches
the typed-argument convention used by `sdkt call` and `sdkt invoke`.

`u128` and `i128` accept decimal integers within their full 128-bit ranges.
Their decoded JSON values are decimal strings, preserving all digits.
`bytes` accepts hexadecimal pairs without a `0x` prefix, with either hex
letter case. Surrounding whitespace is trimmed; empty input (`bytes:`)
encodes empty bytes. Leading zero bytes are preserved, and decoded JSON
uses lowercase hex. Pair parsing matches the runtime typed-argument parser,
including its acceptance of a leading `+` in a pair (`bytes:+f` encodes `0f`).

### Unsupported (clear failure)

Other types — `Vec`, `Map`, `Option`, `Result`, UDTs — are rejected with an
error listing the supported types. Malformed values (bad or out-of-range
numbers, invalid bools, invalid strkeys, odd-length or invalid hex) fail
with a message naming the offending value.

## Generate client

`sdkt generate client` produces deterministic, typed Rust call builders from
the ContractSpec of a compiled Soroban contract WASM. The command is fully
offline — it reads a local `.wasm` artifact and emits Rust source.

```bash
# Print the generated client to stdout
sdkt generate client target/wasm32-unknown-unknown/release/my_contract.wasm

# Write it to a file
sdkt generate client contract.wasm --output src/client.rs

# Generate a partial client, skipping unsupported functions
sdkt generate client contract.wasm --skip-unsupported --output src/client.rs
```

### Output

Each contract function becomes a `*Call` struct with:
- one typed field per parameter,
- a `NAME` constant with the contract function name,
- an `args()` method encoding the call as `TYPE:VALUE` strings — the same
  typed-argument convention accepted by `sdkt call` and `sdkt invoke`,
- a `<Function>Output` type alias mirroring the return type.

The output is deterministic: the same input WASM always produces
byte-identical source, suitable for checking generated files into a repo.

### Supported types (core subset)

Parameters and single return values of these primitive types are supported:
`u32`, `i32`, `u64`, `i64`, `bool`, `address`, `string`, `symbol`, `void`
(no return).

### Unsupported (clear failure)

By default, any other type — UDTs, `Option`, `Result`, `Vec`, `Map`, `Tuple`,
`BytesN`, `Val`, multiple return values — aborts generation with an error naming
the function, the type, and the position (parameter or return). Nothing partial
is emitted. A WASM without a `contractspecv0` section is rejected as
"not a Soroban contract".

### Partial generation (`--skip-unsupported`)

Pass `--skip-unsupported` to generate call builders for the supported subset of
functions instead of aborting the entire command:
- Supported functions are generated in original spec order.
- Skipped functions and their specific reasons are listed deterministically in
  the generated file header comment.
- If all functions in the contract are supported, output is identical to the
  default (no-flag) output.
- If all functions in the contract are unsupported, valid non-crashing output is
  produced with an empty `contract_functions()` and an explanatory header.

## Plugin management

A **local, offline-first** plugin store. All operations are local;
there is no hosted registry and no remote source. Plugins are referenced by a
stable `id` declared in their `plugin.toml`.

```bash
sdkt plugin list                                   # list installed plugins
sdkt plugin list --format json                     # JSON output; every plugin subcommand accepts --format json
sdkt plugin init ./path/to/my-rule                 # scaffold a new audit rule project
sdkt plugin show <id>                              # show a plugin's metadata
sdkt plugin install ./path/to/artifact.wasm        # install from a local file
sdkt plugin remove <id>                            # remove (idempotent)
sdkt plugin update <id> ./path/to/artifact.wasm    # local-only update
sdkt plugin pack ./path/to/plugin-dir --output ./myrule.sdktplugin              # pack into .sdktplugin bundle
sdkt plugin pack ./path/to/plugin-dir --secret-key ./secret.key                 # sign bundle with Ed25519 secret key (32 bytes, raw)
sdkt plugin verify-bundle ./bundle.sdktplugin                                                # verify unsigned bundle integrity
sdkt plugin verify-bundle ./bundle.sdktplugin --public-key ./pubkey.key                      # verify signed bundle against a specific public key
sdkt audit contract.rs --rules <id>                # resolve id → stored artifact
```

`sdkt audit --rules <id>` resolves a plugin `id` to its stored artifact and runs
the existing loader; passing a filesystem path keeps the legacy behavior.
Installing a `native` plugin prints a warning: native plugins run **unsandboxed**
(unmodified behavior). See `docs/plugins/plugin-authoring.md` for the `plugin.toml`
schema and the install-validation rules.

Store root precedence (lowest → highest): `<cwd>/.sdkt/plugins`,
`<config-dir>/sdkt/plugins`, `$SDKT_PLUGIN_DIR`.

- `--format json` is supported on all read-style commands, every `plugin` subcommand, and on `diff`, `audit`, `deploy`, `init` for scripting / CI.
- `diff --upgrade-safety` and `deploy --deny-breaking` implement the Upgrade Safety Guard (see `ROADMAP.md`).
- `audit` implements the static-analysis rules (AUTH-001/002/003/004, MOVE-001).
- `audit --list-rules` discovers all registered built-in rules (with id, severity, and description). Supports `--format json` and does not require a source path argument.
- **Mainnet safety.** Mutating commands (`tx submit`, `invoke`, `deploy`, `project deploy`) refuse to target mainnet unless you explicitly select the network — via `--network-profile`, `--rpc-url`, or `--network-passphrase`. A testnet-default passphrase pointed at a mainnet endpoint is rejected before any request is sent, protecting against signing for the wrong network.

## Error Handling

1. All subcommands return `Result<(), Box<dyn std::error::Error>>`.
2. Library-level errors (`RpcError`, `DecodeError`, `WasmError`) are bubbled up to the CLI.
3. The CLI uses `eprintln!` to print human-readable errors and exits non-zero on fatal errors to ensure correct bash piping behavior.
