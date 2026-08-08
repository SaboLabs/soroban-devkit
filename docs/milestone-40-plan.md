# Milestone M40 — Plugin Ecosystem (Local Store & Distribution)

> Status: **Scheduled (post-M39, v2.5.0).** Authorized from the ROADMAP.md §6
> "Future Work (unscheduled backlog)" item **"Plugin ecosystem / marketplace"**.
> This milestone does NOT introduce a hosted registry or a marketplace server; it
> builds the local, offline-first management layer on top of the existing
> M17–M19 plugin system.

## Objective

Turn the existing path-based plugin loader (`sdkt audit --rules <path>`) into a
managed, identity-addressable plugin ecosystem that a user can install, list,
update, remove, and invoke by a stable plugin id — entirely offline, with no
hosted service. The existing `RuleRegistry`, native (`plugins` feature) and WASM
(`wasm-plugins` feature) loaders are reused verbatim; M40 adds the *management*
layer around them.

This is the minimal, reliable slice of the "marketplace" backlog item: a local
plugin store + metadata format + install/remove/list/update CLI + identity-based
invocation. It deliberately stops short of a remote index or hosted registry
(see Non-Goals and Deferred).

## Problem statement

Today a plugin is only usable by passing its absolute/relative filesystem path
to `sdkt audit --rules`. There is:

- no stable plugin identity (only a file path),
- no plugin metadata (author, version, ABI, description, capabilities),
- no local store (plugins live wherever the user dropped the `.so`/`.wasm`),
- no install/remove/update lifecycle,
- no version policy / compatibility gate beyond the raw ABI major check,
- no way to discover what is installed or to invoke a rule by id.

This blocks the ecosystem goal: authors cannot publish a named, versioned rule
that users can install and trust, and users cannot manage a set of plugins
without hand-curating paths. M40 closes exactly this management gap.

## Existing architecture reused (do NOT replace)

| Existing (M17–M19) | Reuse in M40 |
|---|---|
| `RuleRegistry` (`sdkt-audit::registry`) | Unchanged. M40 never alters registration semantics. |
| Native loader `load_and_register` (`sdkt-audit::plugin_loader`, feature `plugins`) | Reused to load a stored `.so`/`.dylib`/`.dll` by path. |
| WASM loader `load_and_register_wasm` (`sdkt-audit::plugin_loader_wasm`, feature `wasm-plugins`) | Reused to load a stored `.wasm` by path. |
| C-ABI `plugin_abi` (`SDKT_AUDIT_ABI_MAJOR`/`MINOR`) | Reused as the trust/compatibility anchor; M40 adds a metadata `abi_major` field and gates on it. |
| WASM ABI `plugin_abi_wasm` | Reused; M40 reads `sdkt_plugin_id`/`sdkt_plugin_abi_version` from the loaded artifact for metadata. |
| `AuditRule` trait + `register_rule!` | Unchanged. |
| CLI `Audit { rules: Vec<String> }` | Extended: entries may be plugin ids (resolved via the store) in addition to raw paths. |
| `sdkt-audit-example-rule` crate | Reused as the reference plugin packaged into the store for tests/docs. |

## Exact deliverables

1. **Plugin metadata format** — a TOML manifest `plugin.toml` per installed
   plugin, stored under `<store>/<plugin-id>/plugin.toml`, with:

   ```toml
   id = "example-rule"            # stable, namespaced id (author/name recommended)
   name = "Example Rule"
   version = "1.0.0"             # semver
   author = "naninu123"
   description = "Reference audit rule."
   kind = "wasm" | "native"      # matches artifact type
   artifact = "example_rule.wasm"# filename inside the plugin dir
   abi_major = 1                 # must match host SDKT_AUDIT_ABI_MAJOR
   abi_minor = 0
   ```

   Parsed by a new `sdkt-audit` module `plugin_store::metadata` (no new crate).

2. **Local plugin store** — a directory `<store-root>/<plugin-id>/` containing
   `plugin.toml` + the artifact. Store root resolution (precedence, lowest→highest):
   `SDKT_PLUGIN_DIR` env → `<config-dir>/sdkt/plugins` (XDG/`dirs`-based) →
   `<cwd>/.sdkt/plugins`. Reuses the existing config-dir conventions already used
   by `NetworkConfig`.

3. **`sdkt plugin` CLI subcommand** (new, additive) with:
   - `sdkt plugin list` — installed plugins (id, version, kind, from metadata).
   - `sdkt plugin show <id>` — full metadata.
   - `sdkt plugin install <path-or-source>` — copies a local `.so`/`.dylib`/`.dll`/`.wasm`
     + a provided/explicit `plugin.toml` into the store, validates `abi_major`,
     and refuses to install if `abi_major != SDKT_AUDIT_ABI_MAJOR`.
   - `sdkt plugin remove <id>` — deletes the plugin dir (with confirmation-free
     idempotent no-op if absent).
   - `sdkt plugin update <id>` — reserved: with local-source argument only
     (offline). Remote update is deferred (see Non-Goals); the command exists but
     documents that remote sources are out of scope for M40.

4. **Identity-based invocation** — `sdkt audit --rules <id>` now resolves a
   plugin id against the store; raw paths still work unchanged. Resolution order:
   store id → existing raw-path behavior.

5. **Trust/validation on install** — M40 adds metadata verification on top of the
   existing ABI gate:
   - `abi_major` must equal `SDKT_AUDIT_ABI_MAJOR` (hard fail).
   - `kind` must match the artifact extension.
   - artifact must load successfully via the existing loader (dry-run) before
     committing to the store.
   - No signature/checksum verification is introduced in M40 (deferred; see
     Security model).

6. **Offline behavior** — every operation is fully offline. No network calls,
   no index fetch, no telemetry. `install` only accepts a local path or a
   user-supplied `file://` source.

7. **Docs** — extend `docs/plugin-authoring.md` with a "Publishing & Installing"
   section; add `docs/cli.md` `plugin` command reference; note in `RELEASE_READINESS.md`
   that the plugin store is local-only in M40.

## Explicit non-goals (M40)

- NO hosted registry / marketplace server.
- NO remote index, discovery API, or `https://` plugin sources.
- NO crates.io publishing of plugins (the example rule stays a workspace crate,
  not published as a plugin distribution).
- NO plugin signature / code-signing / checksum verification (trust model below).
- NO sandboxed-plugin *permission* UI beyond the existing Extism deny-by-default
  boundary (unchanged).
- NO changes to the `AuditRule` trait, `RuleRegistry`, or the C/WASM ABIs.
- NO version bump, tag, or release cut (release follows the standard process).

## Security / trust model

- The WASM sandbox boundary (Extism, deny-by-default FS/network, no `unsafe` in
  the loader) is preserved exactly.
- Native plugins remain unsandboxed (unchanged M18 behavior); M40 does not weaken
  this and surfaces a clear warning when installing a `kind = "native"` plugin
  ("native plugins run unsandboxed; only install from trusted sources").
- Trust in M40 is **provenance-by-path**: the user installed the artifact from a
  local file they obtained out-of-band. No third-party trust is assumed.
- Install-time `abi_major` gating prevents loading incompatible/foreign binaries.
- Future signature verification is explicitly deferred; documented as the next
  trust layer, not part of M40.

## Plugin metadata / package format

Single `plugin.toml` + one artifact file per plugin directory. No archive format
required in M40 (install copies the artifact + manifest). A future `.sdktplugin`
bundle is deferred.

## Discovery / index mechanism

Local only: enumeration of `<store-root>/*/plugin.toml`. No remote discovery.
`sdkt plugin list` is the discovery surface.

## Install / update / remove behavior

- **install**: validate (abi_major, kind/ext match, dry-run load) → copy artifact
  + manifest into `<store>/<id>/` → register success message. Idempotent on
  re-install (overwrites same id).
- **remove**: delete `<store>/<id>/`. No network, no confirmation prompt (CLI
  automation-friendly), but errors if other state references it (none in M40).
- **update**: local-source only (`sdkt plugin update <id> <path>`); remote update
  deferred. The command is present but rejects remote sources with a clear
  "out of scope in M40" message so the UX is forward-compatible.
- **show/list**: read-only metadata reads.

## Compatibility / versioning policy

- Plugin `abi_major` must equal host `SDKT_AUDIT_ABI_MAJOR` (reuse M18/M19 rule).
- Plugin `version` is semver, informational in M40 (no auto-update resolution).
- Host `sdkt` version and plugin `version` are independent; only `abi_major`
  gates loading, exactly as today.

## Offline behavior

All `sdkt plugin *` and id-resolved `sdkt audit --rules <id>` work with zero
network. Install sources are local paths only.

## CLI UX

```
sdkt plugin list
sdkt plugin show <id>
sdkt plugin install <local-path> [--id <id>] [--force]
sdkt plugin remove <id>
sdkt plugin update <id> <local-path>     # local-only; remote rejected
sdkt audit <path.rs> --rules <id>        # id resolves via store; path still works
```

## Files / modules expected to change (docs/scheduling only in THIS phase;
code changes are the implementation phase, not yet done)

- `crates/sdkt-audit/src/plugin_store.rs` (NEW) — store resolution, metadata
  parse, install/remove/list, id→path resolution. Reuses `plugin_loader` /
  `plugin_loader_wasm` for dry-run validation.
- `crates/sdkt-audit/src/lib.rs` — export `plugin_store` module.
- `crates/sdkt-cli/src/main.rs` — add `Plugin` subcommand; extend `Audit.rules`
  to resolve plugin ids.
- `crates/sdkt-cli/Cargo.toml` — `dirs` dependency (if not already present) for
  config-dir resolution; no new feature flags required (store works with both
  `plugins`/`wasm-plugins` features as today).
- `docs/plugin-authoring.md` — publishing/installing section.
- `docs/cli.md` — `plugin` command reference.
- `RELEASE_READINESS.md` — note local-only plugin store.
- `ROADMAP.md` — schedule M40 (this planning phase).

## Unit / integration / CLI tests

- `crates/sdkt-audit/tests/plugin_store_test.rs`:
  - metadata parse round-trip (valid + malformed rejects),
  - store resolution precedence (env > config-dir > cwd),
  - install copies + validates abi_major (reject mismatch),
  - install rejects kind/ext mismatch,
  - remove idempotent,
  - id→path resolution hits store then falls back to raw path.
- `crates/sdkt-cli/tests/plugin_cli_test.rs` (hermetic, `SDKT_PLUGIN_DIR` temp):
  - `plugin list` empty → after install shows entry,
  - `plugin show <id>` prints metadata,
  - `plugin install <local.wasm>` then `audit --rules <id>` loads it,
  - `plugin remove <id>` then `audit --rules <id>` falls back to path-error.
- Reuse the existing `sdkt-audit-example-rule` artifact (built as `.wasm`/`.so`
  in CI with the relevant feature) as the test fixture.

## Validation gates

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace`
- Docker `sdkt --help` smoke (no behavior change to distribution).
- Manual: `sdkt plugin install` of the example rule, `sdkt audit --rules <id>`
  on a fixture, `sdkt plugin remove`.

## Release / version policy

- M40 is implemented on `feat/milestone-40`, merged to `main`, then released via
  the existing process (version bump → tag → push tag → release.yml).
- NO version bump or tag is performed in the planning phase.
- The feature is additive; default behavior of `sdkt audit --rules <path>` is
  unchanged.

## Completion criteria

1. `sdkt plugin list/show/install/remove` work fully offline against the local
   store.
2. `abi_major` mismatch and kind/ext mismatch are rejected at install.
3. `sdkt audit --rules <id>` resolves a store plugin; raw paths unchanged.
4. All unit/integration/CLI tests green under the four gates above.
5. Docs updated; `RELEASE_READINESS.md` notes local-only store.
6. No new feature flags required; no hosted component introduced.

## Explicitly deferred to future backlog

- Hosted registry / index server.
- Remote `https` plugin sources and `sdkt plugin update` from a remote.
- Plugin signing / checksum verification / trust attestation.
- `.sdktplugin` bundle archive format.
- Plugin *marketplace* UI / web portal.
- crates.io distribution of third-party plugins.
- Multi-author namespace governance.

These remain in ROADMAP.md §6 "Future Work (unscheduled backlog)" and are NOT
part of M40.
