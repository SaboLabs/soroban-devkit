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

├── network
│   ├── add <name>           [--rpc-url <URL>] [--passphrase <PASS>] [--friendbot <URL>] [--description <TEXT>]
│   ├── list
│   ├── show <name>          [--format json|pretty]
│   └── remove <name>

├── build                     Compile Rust contracts in the workspace into WASM artifacts

├── lock                      Generate or inspect `sdkt.lock` (M34.1, M35.2)
│   ├── generate              Write `sdkt.lock` from current build artifacts (run `sdkt build` first)
│   ├── verify                Verify `sdkt.lock` against on-disk contract artifacts AND package
│   │                         dependencies (lock matches manifest, git commits, path existence).
│   │                         Advisory and non-fatal: prints `✓ lock file verified` /
│   │                         `✓ package dependencies verified` or lists every drift.
│   └── show                  Print `sdkt.lock` contents

├── package                   Validate local package manifests (M35.0)
│   ├── validate              Offline-validate `[package]` metadata + local `[dependencies]`
│                             path graph (no network/registry; git/* sources rejected)
│   ├── fetch                 Fetch deps into `.sdkt-cache` (M35.1): local path passthrough,
│                             git clone/checkout. `--force` updates. Never builds.
│   ├── update                Synchronize deps (M36.0): refresh git deps to latest available
│                             commit and rewrite `sdkt.lock`. `rev` pinned; `tag`/`branch`
│                             update on drift. `--check` reports; `--dry-run` previews.
│   ├── pack                  Bundle the resolved project into a portable offline artifact
│                             (M38): manifest + lock + cached git checkouts. `--out`, `--format`.
│   └── publish               Validate publish readiness (M38, `--dry-run` only, read-only);
│                             detects missing cache, lock drift, integrity mismatch.

### Synchronizing dependencies (M36.0)

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

### Version-constrained dependencies (M37)

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

### Offline packaging & publish readiness (M38)

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
explicitly opt-in and is rejected because M38 defines no registry source; the
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
│   Contracts declare dependencies via `depends_on` (canonical, M34.2) or the
│   legacy `deploy_after` field in `[contracts.<alias>]`; both are merged. Build,
│   deploy, and `sdkt lock generate` share one resolver, so order is deterministic.
│   Invalid graphs (unknown/self/duplicate dependency, cycle, duplicate name)
│   fail fast with a clear error.
│
└── deploy
    ├── --wasm <file>
    ├── --salt <salt>
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

## Plugin management (M40)

M40 introduces a **local, offline-first** plugin store. All operations are local;
there is no hosted registry and no remote source. Plugins are referenced by a
stable `id` declared in their `plugin.toml`.

```bash
sdkt plugin list                                   # list installed plugins
sdkt plugin show <id>                              # show a plugin's metadata
sdkt plugin install ./path/to/artifact.wasm        # install from a local file
sdkt plugin remove <id>                            # remove (idempotent)
sdkt plugin update <id> ./path/to/artifact.wasm    # local-only update
sdkt audit contract.rs --rules <id>                # resolve id → stored artifact
```

`sdkt audit --rules <id>` resolves a plugin `id` to its stored artifact and runs
the existing loader; passing a filesystem path keeps the pre-M40 behavior.
Installing a `native` plugin prints a warning: native plugins run **unsandboxed**
(unchanged M18 behavior). See `docs/plugin-authoring.md` for the `plugin.toml`
schema and the install-validation rules.

Store root precedence (lowest → highest): `<cwd>/.sdkt/plugins`,
`<config-dir>/sdkt/plugins`, `$SDKT_PLUGIN_DIR`.

- `--format json` is supported on all read-style commands and on `diff`, `audit`, `deploy`, `init` for scripting / CI.
- `diff --upgrade-safety` and `deploy --deny-breaking` implement the Milestone 14 Upgrade Safety Guard (see `ROADMAP.md`).
- `audit` implements the Milestone 13 static-analysis rules (AUTH-001/002/003, MOVE-001).
- **Mainnet safety (M39).** Mutating commands (`tx submit`, `deploy`, `project deploy`) refuse to target mainnet unless you explicitly select the network — via `--network-profile`, `--rpc-url`, or `--network-passphrase`. A testnet-default passphrase pointed at a mainnet endpoint is rejected before any request is sent, protecting against signing for the wrong network.

## Error Handling

1. All subcommands return `Result<(), Box<dyn std::error::Error>>`.
2. Library-level errors (`RpcError`, `DecodeError`, `WasmError`) are bubbled up to the CLI.
3. The CLI uses `eprintln!` to print human-readable errors and exits non-zero on fatal errors to ensure correct bash piping behavior.
