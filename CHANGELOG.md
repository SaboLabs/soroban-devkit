# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **128-bit integer and byte encoding.** `sdkt encode` accepts decimal `u128`/`i128` and hexadecimal `bytes`, matching the runtime typed-argument parser on valid inputs. Invalid or out-of-range values fail with type/value diagnostics, and malformed Unicode byte input is rejected without panicking (#102).
- **Discovery of audit rules via `sdkt audit --list-rules`.** Added `--list-rules` flag to `sdkt audit` to discover all registered built-in rules with their ID, severity, and concise description without requiring a source file path. Supports `--format json` emitting a structured JSON array of rule metadata (`id`, `severity`, `description`), with deterministic registry ordering and immunity to `--disable` filters (#182).
- **Automatic execution of installed plugins during `sdkt audit`.** Installed plugins in the local plugin store run automatically during `sdkt audit` without requiring `--rules <id>`. `--rules` remains available for explicit rule selection and ad-hoc artifacts, and `--no-plugins` allows skipping installed plugins. Incompatible plugin ABIs emit a warning and are skipped without aborting the audit, and audits with plugins loaded display a rules summary line (#80).
- **Constructor argument support in `sdkt deploy`.** Added repeated `--arg <type:value>` flag to `sdkt deploy` accepting typed arguments (`u32`, `i32`, `u64`, `i64`, `u128`, `i128`, `bool`, `string`, `symbol`, `bytes`, `address`, or pre-encoded base64 ScVal). When constructor arguments are present, deployment uses Soroban's `HostFunction::CreateContractV2(CreateContractArgsV2)` for creation-time constructor execution. Zero-argument deployments preserve the existing `CreateContract` V1 path. Added `build_create_contract_v2_tx`, `build_create_contract_v2_tx_with_data`, and `build_create_contract_v2_tx_with_data_and_auth` builders to `sdkt-xdr` and updated auth entry parsing to handle `SorobanAuthorizedFunction::CreateContractV2HostFn` (#58).
- **Configurable event query ranges.** `sdkt events` accepts `--start-ledger <N>` and `--end-ledger <N>` flags to query specific ledger ranges. Ranges default to the prior 1000-ledger window if omitted, single bounds query from/to latest or relative windows, and inverted ranges (`start > end`) are rejected before RPC dispatch (#57).
- **Network-aware fee estimation for `sdkt tx build`.** `tx build` hard-coded a 100-stroop inclusion fee and never consulted the network, and emitted the envelope with no Soroban footprint at all, so the documented `tx build` → `tx sign` → `tx submit` workflow produced transactions the network rejects. Whenever the build already has a network to talk to — the operator named one (`--network-profile` / `--rpc-url` on `tx`), or the sequence is being auto-fetched — `build` now simulates the invocation and adopts the reported `minResourceFee` and footprint, mirroring `invoke`. A build that pins `--sequence` and names no network stays fully offline and unchanged, but now warns that the fee is inclusion-only. An explicit `--fee` still takes precedence and costs no network round trip. The simulate-and-adopt step is shared with `invoke` via the new `sdkt_rpc::simulate_invoke` (#76).
- **`sdkt plugin init`.** Scaffold a new audit rule project (standalone crate, derived rule id, native/WASM ABI files, `plugin.toml`, README, and unit tests) derived from `crates/sdkt-audit-example-rule`, so a plugin author goes straight to `cargo build --release --features plugins` without hand-copying the reference implementation (#95).
- **JSON output for plugin commands.** Every `sdkt plugin` subcommand (`list`, `show`, `install`, `remove`, `update`, `pack`, `verify-bundle`) accepts `--format json`. Stdout carries only the JSON document; the native-plugin warning and the unsigned-bundle note stay on stderr. (#54).
- **Actual fee and operation count in `sdkt tx inspect`.** Settled transactions now decode `resultXdr` and `envelopeXdr` to report the fee charged and number of operations in pretty and JSON output; pending/not-found or malformed XDR keeps those fields absent (#71).
- **Deployment fee breakdown in deploy output.** `sdkt deploy` exposes internal simulation fee calculations (`upload_fee`, `create_fee`, and `total_fee`) in `DeployResult` and displays them in both pretty and JSON output formats (#55).
- **Real Soroban contract deployment.** `sdkt` can deploy Wasm contracts to a live network (upload Wasm, create the contract instance, and report the resulting contract ID), replacing the previous placeholder/stub path.
- **Auto-generated deployment salt.** Deploy flows generate a salt when the operator does not supply one, so routine deployments no longer require a hand-crafted hex salt.
- **Contract TTL extension.** Operators can extend a contract instance’s TTL via the CLI/storage helpers.
- **Contract state read by LedgerKey.** Read on-chain contract state using a LedgerKey, for inspection and tooling workflows.
- **Friendbot funding.** Identity/account helpers can request Friendbot funding on supported test networks.
- **Read-only contract call.** `sdkt` can perform a read-only contract call against a live contract.
- **ABI-aware result decoding for contract call and simulation.** Call and transaction-simulation paths can decode results using contract ABI metadata when available.
- **Real account inspection.** Account inspection talks to the network and reports live account state instead of a stub.
- **State-changing contract invoke.** `sdkt invoke` submits state-changing contract invocations (with JSON output improvements noted under Fixed).
- **`sdkt doctor`.** New diagnostics command that checks local toolchain/environment readiness (CI- and Windows-safe checks).
- **Client generation and XDR encoding.** CLI support for generating clients and encoding XDR values, including `symbol:VALUE` as a Soroban `ScVal::Symbol` (values longer than 32 bytes fail with a clear error).
- **Local plugin ecosystem and signed plugin bundles.** Local plugin loading/ecosystem support, plus a signed, reproducible plugin bundle format with e2e/compatibility coverage.
- **On-chain inspection tooling.** Enriched on-chain contract inspection, upgrade-safety verification, live-contract ABI for events decode, and on-chain ABI for storage decode.
- **`tx simulate --abi-contract`.** Transaction simulation can now fetch a deployed contract's on-chain WASM and decode the primary result with its ABI, matching the existing `--abi-contract` support in `events`/`storage`. `--abi` and `--abi-contract` are mutually exclusive.
- **`call --abi-contract`.** Read-only calls can decode the result using the deployed contract's on-chain WASM (`inspect_contract` → `get_wasm_bytecode` → `parse_contract_spec`) instead of requiring a local artifact, matching `events`/`storage`/`tx simulate`. `--abi` and `--abi-contract` are mutually exclusive; local `--abi` and no-flag behavior are unchanged.
- **Windows x86_64 release binary.** Packaged Windows builds are available alongside existing platforms.
- **Website and Web Playground.** Public landing page and a browser-based contract inspector (Web Playground MVP).

### Fixed
- `sdkt project deploy --salt` is no longer silently ignored. The value was discarded and every contract got a random salt, so the same project produced different contract IDs on each run. The salt now uses the same 40-hex-char format and validation as `sdkt deploy --salt` and is forwarded to each deployment. The `"deploy"` string default is gone: omitting `--salt` auto-generates one, as before.
- Updated public landing page content (`website/index.html`) to reflect shipped CLI capabilities, adding contract invocation (`invoke` / `call`), event exploration, storage read/extend, account inspection, and Friendbot funding, updating the workflow to feature `invoke` as the primary execution path, adding a real `invoke` terminal demo, and fixing/expanding documentation footer links (#82).
- `sdkt identity delete` now fails with a not-found error, and exits non-zero, when the identity does not exist, instead of reporting a removal that did not happen. `IdentityStore::remove` returns `StorageError::NotFound` in that case, matching `network remove` (#109).
- `storage extend --ledgers` is documented as the relative TTL it is. The getting-started guides and the `ExtendFootprintParams::extend_to` doc comment described it as an absolute ledger sequence, but the protocol's `ExtendFootprintTTLOp.extendTo` means "at least N ledgers from the last closed ledger". Behaviour is unchanged; a regression test now pins that `N` reaches the operation as given (#110).
- Generated client arguments use the `i64:` type tag for `i64` parameters (#105).
- `sdkt deploy` now reports the file path and read error when its WASM file cannot be read (#49).
- Deploy salt validation distinguishes invalid length from invalid hex, with clearer error messages (#51).
- `sdkt invoke --format json` includes `errorResultXdr` when an RPC submission fails, matching the existing pretty output. The field is `null` when the RPC response has no error result XDR.
- RPC/XDR compatibility restorations for live Soroban LedgerEntry handling, on-chain inspection paths, and contract-instance TTL queries.
- Windows debug stack overflow avoided by running async main on an 8MB-stack thread.
- AUTH-004 audit rule registration/behavior improvements (plus additional AUTH/MOVE unit coverage).

### Changed
- Public docs refreshed for deployment, contributor onboarding, security-model wording, Windows availability, GitBook navigation, and README positioning. Historical internal/planning docs were archived or removed where appropriate.

## [v2.5.0] - 2026-08-08

### Added
- **Release polish & SCF readiness. Containerized distribution via a
  maintained multi-stage `Dockerfile` (+ `.dockerignore`); a conservative
  mainnet-safety guard on mutating RPC commands (`tx submit`, `deploy`,
  `project deploy`) that refuses mainnet unless the operator explicitly selects
  the network; `docs/scf.md` positioning the project for SCF grant tracks;
  refreshed `RELEASE_READINESS.md`; and opt-in `--version` provenance behind the
  new `provenance` feature (default OFF, so builds stay reproducible).
- **Shell completions. New `sdkt completions <shell>` command generates
  completion scripts for `bash`, `zsh`, `fish`, `powershell`, and `elvish`
  (via `clap_complete`). Documented in README and `docs/cli.md`.
- **CLI integration tests. New `crates/sdkt-cli/tests/cli_integration.rs`
  covers `--help`, `--version`, `completions`, the full `network`
  add/list/show/remove lifecycle with `--format json`, invalid-argument handling,
  and offline commands. Tests are deterministic and hermetic (per-test
  `SDKT_NETWORK_DIR`).
- **Supply-chain audit. New `supply-chain` CI job runs `cargo audit`
  directly (no third-party action) on `ubuntu-latest` with `continue-on-error`,
  so dependency advisories, advisory-DB, or network issues never break the
  build. Portable across the existing Linux/macOS/Windows test matrix.
- **Runnable crate doc examples. Added `///` doc examples to
  `sdkt_storage::NetworkProfile` and `sdkt_wasm::parse_metadata` so docs.rs
  renders verified, executable examples.
- **Contract dependency graph resolution** `.sdkt.toml` now supports an
  explicit `depends_on` field per `[contracts.<alias>]` (legacy `deploy_after`
  remains accepted and is merged with it). A single topological sort
  (`resolve_deploy_order`) is the source of truth shared by `sdkt build`,
  `sdkt project deploy`, and `sdkt lock generate`, giving deterministic,
  identical ordering across all three. The resolver validates the graph and
  returns clear, human-readable errors for: unknown dependency, self-dependency,
  duplicate dependency (same dep declared more than once), circular dependency,
  and duplicate contract name (TOML parse error now surfaced via `sdkt` instead
  of silently defaulting to an empty config). New unit tests cover each invalid
  case plus deterministic ordering; new CLI integration tests assert `sdkt build`
  fails fast on invalid graphs.
- **Local package manifest foundation. Foundation for a future
  package registry with **no** network or remote-registry functionality.
  `.sdkt.toml` now accepts a `[package]` section (`name`, `version`,
  optional `description`) and a `[dependencies]` table of **local path-only**
  references (`path = "..."`). New `sdkt package validate` validates the
  manifest offline: required `name`/`version` (semver-shaped), local-path-only
  dependencies (non-path sources like `git` are rejected at parse time via
  `deny_unknown_fields`), no self-dependency, existing `path` directories, and
  an acyclic dependency graph. The dependency-graph cycle/duplicate/self checks
  reuse the same Kahn's topological-sort core as contract deploy-order
  resolution (`sdkt_core::package::topo_sort`). Fully backward compatible, no
  breaking CLI changes, no version bump/tag/publish, no external services. New
  unit tests cover every validation error; new CLI integration tests cover valid
  and invalid manifests end-to-end.
- **Git dependency sources. Package dependencies now support both
  local `path` and `git` sources, exactly one per `[dependencies.<name>`. Git
  deps take a `git` URL plus exactly one of `tag` / `branch` / `rev`. Validation
  rejects `path` + `git` together, multiple git references, missing/empty git
  URL, unsupported schemes (`https`/`http`/`git`/`ssh` or `git@host:org/repo`),
  empty references, duplicate names, and cycles — reusing the existing resolver
  and shared `topo_sort`. New `sdkt package fetch` materializes dependencies
  into a deterministic `.sdkt-cache` via the system `git` CLI (no registry, no
  auth helpers, never builds; `--force` updates existing checkouts). A
  `DependencyFetcher` trait abstracts acquisition so a future registry source
  plugs in without touching callers. `sdkt.lock` now records each dependency's
  source, git URL, requested reference, and resolved commit SHA (local path
  deps unchanged). `LocalDependency` is retained as a type alias of `Dependency`
  for backward compatibility. Fully backward compatible: no breaking CLI
  changes, no version bump/tag/publish, no external service beyond git fetch.
  New unit tests cover parser/validation/lock serialization and fetch using
  on-the-fly local git repos (no network); new CLI integration tests cover
  validate-accept, validate-reject, and offline fetch end-to-end.
- **Lock dependency resolution & reproducible verification**
  `sdkt.lock` dependency entries now record the full resolved state for
  reproducibility: `name`, `source` (`local`/`git`), `original_source` (the
  resolved path or git URL), `git_url`, the requested `resolved_reference`
  (`tag`/`branch`/`rev`), the `commit_sha` resolved at fetch time, the on-disk
  `cache_location`, and an `integrity` hash (`sha256:<hex>` of the cached git
  tree or the local directory tree, computed offline). `sdkt package fetch`
  writes these fields into `sdkt.lock` (updating it in place when one already
  exists, preserving contract artifacts). New `verify_dependencies` reports a
  structured [`DepVerifyReport`] instead of panicking: it confirms the lock
  matches the manifest (source + reference), local `path` deps still exist,
  and cached git checkouts resolve to the locked commit. `sdkt lock verify`
  now verifies package dependencies **in addition to** contract artifacts —
  printing `✓ package dependencies verified` when consistent, or listing
  every drift (missing-in-lock, source-changed, reference-changed,
  path-missing, cache-missing, commit-mismatch, integrity-mismatch,
  not-in-manifest). Fully offline and advisory (never blocks the build). No
  registry, no network, no new dependency-graph or validation code — it reuses
  the existing [`crate::package::validate_dependencies`] and the
  `GitFetcher`/`git_cache_key` infrastructure. New unit tests cover
  dependency lock round-trip (incl. new fields), consistent verification, path
  missing, source/reference drift, not-in-manifest, and git cache-mismatch;
  new CLI integration tests cover `lock verify` dependency reporting and
  `package fetch` writing reproducible lock entries.

- **Package update & synchronization. New `sdkt package update`
  closes the package loop (`validate → fetch → update → verify`). It resolves
  each git dependency's currently available commit via `git ls-remote`
  (offline for local URLs; contacts only the declared git remote when a
  network ref is required), then refreshes `tag`/`branch` deps to the latest
  commit, updates the cache, and rewrites `sdkt.lock` (`commit_sha`,
  `resolved_reference`, `cache_location`, `integrity`) — preserving contract
  artifacts, hashes, and deploy order. `rev` deps are reported `pinned` and
  never updated; local `path` deps are reported `unchanged`. Flags:
  `--check` (report-only, exits 0), `--dry-run` (preview, touches nothing),
  `--format pretty|json` (JSON emits `checked`/`updated`/`unchanged`/`changes`).
  Clear, non-panicking errors for missing cache, git unavailable, invalid
  manifest, missing lock, detached branch, unknown reference, and network
  failure. Reuses `GitFetcher`, `git_cache_key`, `lock_dependencies`,
  `verify_dependencies`, and the existing cache/lock infrastructure — no new
  dependency-graph, topo-sort, validation, hashing, or git-cache code. New
  unit tests cover branch/tag/rev update, dry-run, check mode, JSON output,
  lock refresh, unchanged dep, update-after-fetch, missing cache, and invalid
  new CLI integration tests cover `package update`, `--check`, `--dry-run`,
  `--format json`, and offline local-path behavior.

- **Dependency version resolution. Git dependencies may now declare an
  optional semver `version` constraint (e.g. `version = ">=1.0, <2"`) instead
  of a fixed `tag`/`branch`/`rev`. When a constraint is set without an explicit
  ref, `sdkt package fetch` / `sdkt package update` resolve the **highest
  remote tag that satisfies the constraint** (via `git ls-remote --tags`,
  offline for local remotes) and materialize / lock that exact tag. An explicit
  `tag`/`branch`/`rev` still takes precedence and the constraint is ignored
  (mirroring how `rev`/`path` deps bypass version resolution). `sdkt package
  update --check` reports `constraint unsatisfied` when no tag matches; the
  lock records the resolved `version` for audit. Reuses the existing
  `git_cache_key` / `GitFetcher` / `lock_dependencies_resolved` infrastructure
  and adds a single pure `VersionResolver::best_version_match` (semver crate)
  as the one source of truth for constraint matching — no dependency-graph,
  validation, or fetch logic duplicated. New unit tests cover resolver
  selection, update detection, up-to-date, and unsatisfied-constraint; new CLI
  integration tests cover fetch/update picking the highest satisfying tag and a
  clear error on an unsatisfiable constraint.
- **Offline packaging & publish readiness. New `sdkt package pack`
  bundles the fully resolved project (`.sdkt.toml` + `sdkt.lock` + cached git
  checkouts under `.sdkt-cache/git/<key>`) into a portable offline artifact,
  either a compressed tarball (`--format tar.zst`) or a directory tree
  (`--format dir`), into `--out` (default `./dist`). Each artifact embeds a
  `package.json` `PackageBundle` descriptor recording the package name/version,
  chosen format, the `sdkt.lock` sha256, and per-dependency `source`/`git_url`/
  `commit_sha`/`integrity`/`cache_key`/`version` so the bundle is verifiable
  offline. New `sdkt package publish --dry-run` validates publish readiness
  (read-only) reusing the existing `verify_dependencies` / manifest-validation /
  cache / integrity primitives; it detects missing cache, lock drift, integrity
  mismatch, commit mismatch, reference change, and invalid package state.
  `--broadcast` is opt-in and rejected because this release defines no registry source
  (no network, nothing published). New `unpack` + `verify_bundle_equivalence`
  prove a reconstructed tree reproduces the original lock sha256 and per-git
  integrity exactly. Adds `tar` / `zstd` deps. New unit tests cover pack
  round-trip (dir + tar.zst), unknown-format rejection, and publish readiness
  (ready / missing-cache drift); new CLI integration tests cover `pack`,
  `pack --out`, format handling, pack→unpack→lock/integrity equivalence,
  `publish --dry-run` success, `publish --dry-run` failure on drift, and fully
  offline operation. No version bump, no tag, no publish, no registry server.

### Planned
- Post-2.0 mainnet-focused tooling, SCF grant alignment, and a plugin marketplace.

## [v2.4.0] - 2026-08-07

### Added
- **Network Profiles. Named network profiles let you
  save RPC URL + network passphrase once and reference them from any RPC
  command, instead of repeating full endpoints.

  - **Network Profile Storage (`sdkt-storage`):** new `NetworkStore`
    (honors `SDKT_NETWORK_DIR`) and `NetworkProfile` types with `add` / `get` /
    `list` / `remove` / `exists`. Stored as JSON under the project config dir
    (`~/.config/sdkt/networks`, overridable via `SDKT_NETWORK_DIR`). Validation
    rejects empty names, path separators in names, and empty RPC URLs.
  - **Network Profile CLI (`sdkt network`):** `sdkt network add |
    list | show | remove` manage profiles; `show --format json` for scripting.
    Additive, no breaking changes to existing commands.
  - **Network Profile Integration:** every RPC command now accepts
    `--network-profile <NAME>` plus explicit `--rpc-url <URL>` and
    `--network-passphrase <PASSPHRASE>` override flags. Resolution precedence
    (highest wins): explicit `--rpc-url` / `--network-passphrase` >
    `--network-profile` > `.sdkt.toml` `[network]` > `NetworkConfig::default()`.
    Commands covered: `inspect`, `verify`, `health`, `storage`, `events`,
    `account`, `tx`, `fee`, `wasm`, `deploy`, `project deploy`. Commands without
    the flag behave exactly as before. `tx sign` is excluded (offline signing).

### Changed
- Documentation: `docs/cli.md`, `README.md`, and `docs/examples.md` document the
  `sdkt network` command and the `--network-profile` / `--rpc-url` /
  `--network-passphrase` flags and their precedence.

### Testing
- Pure precedence logic covered by unit tests (`resolver_tests`) in `sdkt-cli`
  (flags > profile > built-in defaults). Integration tests under
  `crates/sdkt-cli/tests/network_cli.rs` cover the `sdkt network` management
  surface and a deterministic profile-not-found path; all tests are CI-safe
  (no live RPC, no internet, no machine-specific config).


## [v2.2.0] - 2026-08-07

### Added
- **Native transaction signing** `sdkt` can now sign Soroban transaction envelopes with a local ED25519 identity — completing the build → sign → submit lifecycle.
  - New `sdkt tx sign` command: signs a base64 `TransactionEnvelope` (or a file containing one) using an identity from the local keystore, appending a `DecoratedSignature`. Fully offline — no RPC, no secret exposure.
  - Flags: `--input <xdr|file>`, `--output <file>` (prints to stdout if omitted), `--identity <name>` (defaults to `default`), `--network testnet|mainnet|futurenet|custom:<passphrase>`, `--format json|pretty`.
  - Core signing library in `sdkt-xdr` (`sign_transaction`, `sign_envelope_with`, `verify_signature`, `Ed25519Signer`, `Network`, `Signer`, `SigningOptions`, `SigningError`) — dependency-clean and reusable by future signers (hardware/remote, deferred).
  - Keystore integration: `sdkt-storage::IdentityStore::load_signing_key(name)` exposes the in-memory `ed25519_dalek::SigningKey` for signing without serializing secrets.

### Changed
- `sdkt tx validate` and `sdkt tx sign` are now first-class `tx` subcommands documented across README, `docs/cli.md`, and `docs/examples.md`.
- Documentation: README, quick-start, CLI reference, and examples now show the complete build → validate → simulate → sign → submit workflow.

### Fixed
- Stabilized the `sdkt-cli` identity lifecycle integration test (`test_cli_identity_lifecycle`), which was intermittently failing under parallel `cargo test --workspace` due to a process-global `HOME` mutation. The test now redirects the keystore via a per-subprocess `XDG_CONFIG_HOME`, eliminating the flakiness without weakening coverage.

### Security
- Signing derives the canonical envelope hash via `stellar_xdr`'s `TransactionEnvelope::hash(network_id)` (version-correct preimage construction) — no hand-rolled `HashIdPreimage`.
- Secret key material is handled only as in-memory `ed25519_dalek::SigningKey` values for the duration of a single sign call. No secret bytes are ever written to logs, `stdout`, `stderr`, or error messages; `SigningError` carries only key-free text.

### Testing
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace` all pass (280 tests, 0 failed).
- New coverage: 12 unit tests in `sdkt-xdr` (network handling, single/double signature append, golden-vector determinism, cross-network divergence, invalid base64/XDR/empty/envelope, invalid/unknown secret key, secret-key roundtrip, wrong-signer rejection) + 10 `sdkt-cli` integration tests for `tx sign` (success → file/stdout/json, success cases, unknown/missing identity, invalid file/base64/envelope, invalid network, unwritable output).

## [v2.1.1] - 2026-08-06

### Fixed
- **Release consistency hardening.. Aligned the workspace version (`[workspace.package].version`) and every crate's pinned version to `2.1.1`. Fixed a packaging drift where `Cargo.lock` still pinned crates to `2.0.0` after the workspace version was bumped.
- Added CI guardrails in `release.yml` so a release tag must exactly match the Cargo workspace version, and the built binary's reported version must match the tag (catches version-drift regressions before publish).
- `docs/*`: refreshed version references and benchmark dates to `2.1.1`.

## [v2.1.0] - 2026-08-06

### Added
- **Transaction Simulation Enhancements . Improved `sdkt tx simulate` to deserialize and display modern RPC metadata:
  - Added support for `restorePreamble` (surfaced when expired state restoration is required).
  - Added support for `stateChanges` tracking.
  - Enhanced human-readable CLI formatting to display these new fields along with operation counts. Backward compatibility with older RPC payloads is fully preserved.
- **RPC Connection Pooling . Replaced one-off HTTP clients with a single persistent, internally pooled `reqwest::Client` in `SorobanRpcClient`. This significantly improves performance during multi-contract orchestrated deployments (`sdkt project deploy`) by preventing socket exhaustion. Introduced configurable `timeout_secs` and `pool_max_idle_per_host` in `NetworkConfig`.
- **Offline command benchmark suite** `scripts/bench_offline.sh` plus a documented regression baseline (`docs/performance.md`).
- **Real-world Soroban compatibility matrix + workflow** `docs/compatibility.md` validates `sdkt` against official `stellar/soroban-examples` contracts; `.github/workflows/compatibility.yml` clones the examples read-only, builds a representative subset to WASM, and runs `sdkt` offline commands against the real artifacts (fails on any non-zero exit).
- **Project scaffolding hardening** generated project `Cargo.toml` resolves cleanly and pins `soroban-sdk` to `21.0.0` for fresh builds.
- **Release hardening** release smoke tests and SHA-256 checksums; GitHub Release distribution of binaries + checksums; tarball-content regression guard; `install.sh` checksum fallback for missing standalone `.sha256` assets
- **Adoption / docs** expanded usage examples and security guidelines, and improved the public install experience.

### Changed
- Workspace version bumped `2.0.0` → `2.1.0`.

## [v2.0.0] - 2026-08-06

### Changed
- **BREAKING: CLI Rename.. The executable binary has been officially renamed from `sdkt-cli` to `sdkt`.
  - Automation scripts, alias configurations, and CI pipelines explicitly expecting `sdkt-cli` must be updated.
  - End-users running `cargo install` will now receive an executable named `sdkt`.
  - Documentation and CI references updated appropriately.
- **WebAssembly (WASM) plugin loading** for `sdkt audit` Phase C. Sandboxed, platform-independent `.wasm` plugins can now be loaded via `sdkt audit <src> --rules <plugin.wasm>`.
- Extism runtime integration via the `wasm-plugins` feature (requires `wasm32-wasip1` target for plugin authors).
- JSON-over-memory WASM ABI boundary ensuring memory safety and isolation.
- **Native dynamic plugin loading** for `sdkt audit` Phase B. Native shared
  libraries (`.so` / `.dylib` / `.dll`) exporting the C-ABI plugin symbols can
  now be loaded at runtime via `sdkt audit <src> --rules <plugin.so>`.
- `sdkt-audit` plugin ABI: `plugin_abi` module with `#[repr(C)]` types
  (`SdktAuditFindingC`, `SdktAuditReportC`), `SDKT_AUDIT_ABI_MAJOR`/`MINOR`
  versioning, and the C-ABI symbol contract.
- `sdkt-audit-example-rule` gains `plugins` and `wasm-plugins` features producing loadable artifacts (`libsdkt_audit_example_rule` and `sdkt_audit_example_rule.wasm`).
- CLI: `--rules` now accepts plugin artifacts (`.so`/`.dylib`/`.dll`/`.wasm`) when built
  with `--features plugins` or `--features wasm-plugins`; a clear error is emitted on a default (plugin-less) build.
  Validates `--rules` paths before reading the source (preserves the
  existing "does not exist" error contract).
- Tests: `plugin_loader` unit tests (ABI pack/unpack, severity mapping, missing
  file rejection) and a `sdkt-cli` integration test that builds the example
  plugin and verifies dynamic rules fire alongside built-ins.

### Changed
- **MSRV increased to `1.88.0`**. Required by transitive dependencies (`darling@0.23.x` via `serde_with`, and `stellar-strkey@0.0.18`), not by internal project code.
- `sdkt-audit` exposes `scan_all_functions_str` (convenience for plugin authors).
- No `AuditRule` public-API change. Default build is byte-for-byte identical to
  v1.0.0 — the `plugins` feature is OFF by default.

### Security
- Dynamic plugins run **in-process**; only load plugins you trust / built
  yourself. ABI major-version mismatch is rejected. See `SECURITY.md`.

## [v1.0.0] - 2026-08-05 — First Stable Release

This is the first stable, semver `1.0.0` release. No new features beyond
the previous release; this release stabilizes the toolkit, unifies the version, and makes
the release pipeline fully green end-to-end.

### Added
- Stable `1.0.0` release tag. The full feature set is now
  considered stable: inspect/decode, storage TTL + analysis, ABI-aware
  decoding, transaction simulate/submit/build, events, account, fee
  estimate, WASM metadata/cache, offline ABI/WASM diff with upgrade-safety
  verdict, static security audit (`sdkt audit`), keystore (`sdkt identity`),
  project scaffolding (`sdkt init`), and deploy with optional
  `--deny-breaking` guard.
- Reusable GitHub composite Action (`.github/actions/sdkt`) for
  `audit` / `upgrade-safety` CI gates.

### Changed
- Workspace version bumped `0.17.0-alpha` → `1.0.0` (single source of truth
  in `[workspace.package]`). All crates and internal path-dependencies now
  pin `1.0.0`.
- `sdkt-audit-example-rule` is now publishable (was `publish = false`), so
  `sdkt-cli`'s optional `plugins` feature resolves on crates.io. Added to the
  release publish order before `sdkt-cli`.
- `release.yml`: `CARGO_REGISTRY_TOKEN` guard moved from a job-level `if:`
  (invalid — GitHub rejects `secrets` in `if:`) to a step-level `if:`.
  Added `workflow_dispatch` for manual dry-run verification. Removed the
  `--allow-dirty` flag from `cargo publish` so a dirty tree fails the release.
- `sdkt-action-ci.yml`: push trigger generalized `feat/milestone-15` → `feat/*`.
- Documentation refreshed: ROADMAP marks v1.0.0 released; `docs/ci-cd.md`
  and the composite Action default to the `v1.0.0` tag.

### Testing
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, and `cargo test --workspace` all pass (219 tests, 0 failed).
- `cargo publish --dry-run --workspace` packages all 8 crates cleanly.

---

## [v0.17.0-alpha] - 2026-08-05 (Plugin System Phase A)

### Added (extensibility, no breaking changes)
- **`RuleRegistry`** in `sdkt-audit`: `register_rule`, `register_builtin_rules`, `registered_rules`, `run_all`. Built-in rules (AUTH-001/002/003, MOVE-001) now register through the registry instead of a hardcoded list; finding order and IDs unchanged.
- **Plugin author API**: stable `AuditRule`, `AuditContext`, `Finding`, plus a `register_rule!` macro and process-wide `register_rule()` for external/plugin rules.
- **`sdkt audit --rules <PATH>`** (repeatable, additive): validates external rule paths and runs all registered rules. Omitted → behavior identical to the previous release. (Phase A: external rules must be compiled into the binary; dynamic loading is Phase B.)
- **Example plugin crate** `sdkt-audit-example-rule` (rule `EXAMPLE-001`) demonstrating the authoring workflow; linked only when `sdkt-cli` is built with the `plugins` feature (off by default).
- **`docs/plugin-authoring.md`**: architecture, rule lifecycle, authoring, registration, and testing guidance.

### Changed
- Workspace version bumped `0.16.0-alpha` → `0.17.0-alpha` (single source of truth).

### Testing
- Added unit tests (registry registration, duplicate de-duplication, ordering, builtin count) and integration tests (registry executes built-ins + external rules, `--rules` accepted/validated, example rule fires under the `plugins` feature). All prior 219 tests preserved.

---

## [v0.16.0-alpha] - 2026-08-05 (Release Engineering & Polish)

### Changed (release readiness, no new features)
- **Unified workspace version** — added `[workspace.package]` as the single source of truth (`version = 0.16.0-alpha`, `edition`, `license`, `authors`, `repository`, `homepage`). All 7 crates inherit via `*.workspace = true`; internal path-dependencies pinned to the unified version. Removes the `0.6.0-alpha` vs `v0.15.0-alpha` drift.
- **CI Action install fixed** — `action.yml` now installs `sdkt` from a real git tag (`cargo install --git ... --tag <sdkt-version>`, default `v0.15.0-alpha`) instead of a never-published crates.io version; swapped deprecated `actions-rs/toolchain` for `dtolnay/rust-toolchain@stable`. Inputs unchanged (backward compatible).
- **Release workflow** — `.github/workflows/release.yml` on `v*` tags: fmt/clippy/test validation, `cargo publish --dry-run` per crate, cross-platform binary build (linux / macOS Intel / macOS Apple Silicon) uploaded to a GitHub Release, and ordered `cargo publish` (needs `CARGO_REGISTRY_TOKEN`).
- **`.gitignore`** — removed the contradictory `Cargo.lock` ignore (it is intentionally tracked; the Action installs with `--locked`).
- **Docs** — `README.md` rewritten (real repo URL, all 13+ subcommands, install, CI link); `docs/cli.md` rewritten to the full current command tree.
- **Panic audit** — replaced `unwrap()`/`expect()` on user-input execution paths (the `fee estimate` manual-arg parser and JSON-serialize `println!` sites) with `?`/`map_err` so malformed input returns a clean error instead of panicking. Internal invariants and test code untouched.

## [v0.15.0-alpha] - 2026-08-05 (CI/CD GitHub Action)

### Added
- **Reusable GitHub composite Action** — `.github/actions/sdkt/action.yml` wraps existing `sdkt` capabilities for CI: `command: audit` runs `sdkt audit <target> --format json` and fails when findings meet `severity-threshold` (default `critical`, so `MOVE-001` warnings never break CI); `command: upgrade-safety` runs `sdkt diff --old-wasm <old> --new-wasm <new> --upgrade-safety --format json` and fails when `compatible == false`.
- **Action self-validation workflow** — `.github/workflows/sdkt-action-ci.yml` exercises the composite Action against the committed WASM fixtures: a breaking diff (`us_old.wasm` → `us_new.wasm`) is asserted to fail, and an identical diff is asserted to pass.
- **Documentation** — `docs/ci-cd.md` with copy-paste workflow examples (audit-on-PR, upgrade-safety-on-release, self-validation) plus install/threshold notes.
- Packaging only: no new crate, no Rust changes, no breaking API changes. Reuses the existing `sdkt audit` and the existing `sdkt diff --upgrade-safety` JSON contracts.

## [v0.14.0-alpha] - 2026-08-05 (Upgrade Safety Guard)

### Added
- **`sdkt diff --upgrade-safety`** — transforms the existing `SpecDiff` into an actionable `UpgradeVerdict`: `breaking_changes` (removed function, changed signature, removed event, removed type) vs `non_breaking_changes` (additions). Pretty + JSON via existing `--format`.
- `sdkt-wasm`: `UpgradeVerdict`, `VerdictChange`, `ChangeKind`, `upgrade_safety()` / `upgrade_safety_wasm()` — all derived from the existing `diff_specs`/`SpecDiff` (no duplicated comparison logic).
- **`sdkt deploy --deny-breaking --old-wasm <deployed.wasm>`** — optional deploy guard that aborts when the upgrade is not backwards-compatible. Off by default; existing `deploy` behavior unchanged when the flag is omitted.
- 6 unit tests (`upgrade_safety`: removed fn, changed signature, removed event, removed type, additions-only, identical) + 5 `sdkt-cli` integration tests (pretty, JSON, `deploy --deny-breaking`).
- Additive, backwards-compatible: new types + new flags only; no breaking API changes.

## [v0.13.0-alpha] - 2026-08-05 (Gap C: Static Security Analysis)

### Added
- **`sdkt audit <path>`** — offline static security analysis of a Soroban contract Rust source. Flags `AUTH-001` (missing `require_auth` on privileged fns), `AUTH-002` (unauthenticated `invoke_contract`), `AUTH-003` (unguarded `initialize`), and `MOVE-001` (suspicious move-after-use, Warning only). Pretty + JSON via existing `--format`.
- New crate **`sdkt-audit`**: `Severity`, `Finding`, `AuditReport`, `AuditRule` trait, `audit_source()` / `audit_source_with()` / `audit_source_with_spec()` (reuses `sdkt-wasm::ContractSpec` for cross-checking). Built-in rules are additive and `--disable`-able.
- 13 unit tests (per-rule positives/negatives, disable, clean, parse-error) + 6 `sdkt-cli` integration tests for `audit`.
- Additive, backwards-compatible: new crate + new CLI subcommand; no breaking API changes; `sdkt-core` remains networking-free.

## [v0.12.0-alpha] - 2026-08-05 (Contract ABI/WASM Diff)

### Added
- **`sdkt diff --old-wasm <A> --new-wasm <B>`** — offline comparison of two contract WASM binaries. Reports added/removed functions, changed function signatures, added/removed events, and added/removed custom types. Pretty + JSON via existing `OutputFormat`.
- `sdkt-wasm`: new `spec_diff` module with `diff_wasm()` / `diff_specs()` and a serializable `SpecDiff` report (per-WASM SHA-256 hash + size context). Reuses the existing `parse_contract_spec` parser.
- 7 unit tests (added/removed/changed functions, events, types, identical-spec, parse-error propagation) + 3 `sdkt-cli` integration tests for `diff`.
- Additive, backwards-compatible: new module + re-exports; no breaking API changes; no new crates.

## [v0.11.0-alpha] - 2026-08-05 (StorageAnalyzer)

### Added (finish StorageAnalyzer)
- **`sdkt storage analyze <contract-id>`** — categorizes a contract's storage into Instance / Persistent / Temporary entries, with a TTL summary and per-entry detail. Pretty + JSON via existing `OutputFormat`.
- `sdkt-storage`: real Instance/Persistent/Temporary classification by decoding the XDR `LedgerKey` (`StorageClass`); `StorageEntry` per-entry detail added to `StorageReport`.
- 5 unit tests (classification round-trips for instance/persistent/temporary/invalid) + 3 `sdkt-cli` integration tests for `storage analyze`.
- Additive, backwards-compatible: `StorageReport` gains `total_entries`, `other_entries`, `entries` (serde-defaulted) — no breaking changes.


## [v0.10.0-alpha] - 2026-08-05 (ABI-aware decoding)

### Added
- **ABI-aware decoding**: `--abi <WASM>` flag on `events`, `inspect`, and `storage check`.
- `sdkt-wasm` `ContractSpec` parser; `sdkt-xdr::decode_event_topics` for event topic/value decoding.
- Real base64 XDR event topic + data-value decoding (previously a no-op stub).
- `sdkt_xdr::scval_from_base64` helper for decoding event payloads.
- ABI functions/events/custom-types display in pretty + JSON output.

### Fixed
- `events --abi` now decodes actual topics/value instead of empty vectors.
- Removed accidentally-committed 4.6 MB `gen_keys` binary; added to `.gitignore`.
- Clippy-clean (`-D warnings`) across workspace.

## [v0.9.0-alpha] - 2026-08-05

### Added
- **WASM tooling**: `sdkt wasm metadata` and `sdkt wasm cache` (info/remove/clear).
- `sdkt-wasm` crate: `ContractSpec` parser for `contractspecv0` / `contractenvmetav0` sections.
- `sdkt deploy` (upload WASM + instantiate) and `sdkt init` project scaffolding engine.
- WASM metadata caching in `sdkt-storage` (`WasmCache`).
- Identity/keystore foundation reused by deploy/init flows.

## [v0.8.0-alpha] - 2026-08-04 (Mutability Foundation)

### Added
- **Transaction simulation**: `sdkt tx simulate` (offline pre-flight via `simulateTransaction`).
- **Transaction submission**: `sdkt tx submit` with optional wait/poll (`submit_and_wait`).
- **Identity / keystore**: `sdkt identity` generate/import/list/show/delete/default (ED25519, `~/.sdkt/identities`).
- **Envelope builder**: `sdkt tx build` (typed arg parsing → base64 XDR envelope).
- **Fee estimation**: `sdkt fee estimate` (RPC dynamic fee or manual base-fee samples).
- Validation module (`sdkt_core::validation`) for offline envelope checks.

## [v0.7.0-alpha] - 2026-08-04

### Added
- **Horizon account enrichment**: `sdkt account` now pulls balances, signers, and associated assets via Stellar Horizon REST.
- **ScVal pretty-print UI**: human-readable ScVal rendering in CLI pretty output (improved readability for events/storage values).

## [v0.6.0-alpha] - 2026-08-04

### Improved
- **Production Readiness**: Hardened `sdkt-rpc` client timeout handling and mapping boundaries.
- **RPC Resilience**: Added internal retry mechanisms to gracefully handle short-lived network interruptions.
- **Documentation Quality**: Maximum structural rustdoc coverage across core workspace crates and updated README with benchmark planning.

### Testing
- Validation completed on workspace boundaries with Clippy strict policies.
- CLI integration tests increased to cover transaction, storage, and account edge-cases.

### Internal
- Workspace DRY cleanups (unified generic `.request()` methods).
- GitHub Actions workflow added to execute formatting and clippy automated checks.

## [v0.5.0-alpha] - 2026-08-04

### Added
- **Transaction Inspection**: `sdkt tx inspect` command to view transaction hash, status, ledger inclusion, and operation counts.
- **Event Explorer**: `sdkt events` command to fetch and list emitted Soroban contract events.
- **Account Inspection**: `sdkt account` command for base level diagnostics of Stellar/Soroban accounts.
- **Generic RPC request abstraction**: `SorobanRpcClient` now exposes a public `request()` method for generic JSON-RPC interactions.
- **Integration Tests**: Comprehensive test suite coverage added for `tx`, `events`, and `account` commands using `assert_cmd`.

### Improved
- **CLI Architecture**: Hardened separation between CLI output formatting and RPC business logic.
- **RPC Abstraction**: Centralized API request formatting and internal error handling mapping inside the RPC crate.
- **Documentation**: Substantial overhaul of README.md and internal documentation outlining workspace boundaries.

### Internal
- Workspace cleanup, dependency deduplication, and module flattening.
- Implemented robust struct-based API boundaries in `sdkt-rpc`.
- Enforced 100% strict test coverage mapping for CLI boundaries.

## [v0.4.0-alpha] - 2026-08-04

### Added
- Initial Soroban RPC inspection tools (`sdkt inspect`).
- Base64 XDR parser via `sdkt-xdr` (`sdkt decode`).
- Storage TTL analysis (`sdkt storage check`).
- Basic workspace architecture and integration testing.

## [Unreleased]

No unreleased changes. AUTH-004 registration and the plugin bundle CLI (`pack` /
`verify-bundle`) shipped in `v2.5.0`.
