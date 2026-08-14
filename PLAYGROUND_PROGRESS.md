# Web Playground Progress

**Component:** Soroban DevKit Web Playground
**Version:** 2.5.0
**Status:** Contract Inspector available in production

---

## Overview

The Web Playground is a browser-based interface to a selected subset of Soroban
DevKit functionality. Its purpose is to let a developer evaluate the toolkit —
and inspect a compiled Soroban contract — without installing anything.

It complements the `sdkt` CLI rather than replacing it. The CLI remains the
primary developer interface and the source of truth for inspection behaviour;
the Playground reuses the CLI's Rust inspection code compiled to WebAssembly, so
the two cannot diverge in parsing behaviour.

The first and currently only functional surface is the **Contract Inspector**.

---

## Architecture

```
Soroban DevKit
├── Rust CLI (sdkt)
│      └── sdkt-wasm ── inspection logic
└── Web Playground
       └── sdkt-playground ── wasm-bindgen binding layer
              └── Web Worker
                     └── browser UI
```

`crates/sdkt-wasm` holds the reusable inspection logic and is shared unchanged by
both front ends:

| Function | Produces |
|---|---|
| `sdkt_wasm::parse_metadata(&[u8])` | SHA-256 hash, byte size, module version, exports, imports, custom-section names |
| `sdkt_wasm::parse_contract_spec(&[u8])` | Contract functions, custom types, events, and environment metadata decoded from the `contractspecv0` / `contractenvmetav0` custom sections |

Both functions are pure — bytes in, owned struct out, with no filesystem,
network, or global state — and their return types already derive Serde
traits, so they cross the JavaScript boundary without additional conversion
code.

`crates/sdkt-playground` is a thin `wasm-bindgen` wrapper containing no
inspection logic of its own. It exposes two functions to JavaScript:

- `inspect_wasm(bytes)` — calls `parse_metadata` (required) and
  `parse_contract_spec` (optional), returning `{ metadata, spec, spec_error }`
- `sdkt_version()` — the toolkit version the build was produced from

The crate is excluded from the root Cargo workspace so that
`cargo build/clippy/test --workspace` remains a pure native build with no
`wasm-bindgen` in the dependency graph. It is compiled separately for
`wasm32-unknown-unknown` by `website/playground/build.sh`, which also runs
`wasm-bindgen` (pinned to 0.2.127) and writes the browser artifacts to
`website/playground/wasm/`.

In the browser, inspection runs inside a Web Worker so parsing never blocks the
UI thread. Uploaded bytes are read into a `Uint8Array`, transferred to the
worker, and parsed inside the worker's WebAssembly instance.

### Repository layout

| Path | Role |
|---|---|
| `crates/sdkt-wasm/` | Inspection logic, shared with the CLI |
| `crates/sdkt-playground/` | `wasm-bindgen` binding layer (browser only) |
| `website/playground/index.html` | Inspector page |
| `website/playground/playground.js` | UI, file handling, result rendering |
| `website/playground/playground.css` | Styles |
| `website/playground/worker.js` | Web Worker that loads the WASM runtime |
| `website/playground/build.sh` | Reproducible artifact build |
| `website/playground/wasm/` | Generated browser artifacts (committed) |

---

## Current Capabilities

The Contract Inspector supports:

- WASM upload by file picker or drag-and-drop
- Inspection entirely inside the browser
- Module metadata — SHA-256 hash (with copy-to-clipboard), byte size, module
  version, and inspection duration
- Exports and imports
- Custom-section names
- Contract specification, when the module carries one:
  - functions, with each parameter's name and type, and return types
  - custom types, with their kind (struct, union, enum, error enum) and members
  - events
  - doc comments, where the contract declares them
- Structured error states with plain user-facing messages
- Reset and re-upload
- Responsive layout, verified down to a 390px viewport

Navigation entries for Analyze, XDR, Diff, and Health are present but marked
unavailable; those surfaces are not implemented.

---

## Security & Privacy

Inspection is entirely local. Uploaded bytes exist only as a `Uint8Array` on the
page and inside the Web Worker's WebAssembly instance. There is no upload
endpoint, no analytics, and no telemetry in the Playground code.

An inspection triggers no network activity. The only requests the application
makes are for its own static assets: the page, its stylesheet, the worker script,
and the WebAssembly runtime. No request carries a body, none targets an external
origin, and contract bytes never appear in a request URL.

This property is covered by the end-to-end suite, which records every request and
additionally instruments `fetch`, `XMLHttpRequest.send`, `navigator.sendBeacon`,
and `WebSocket` while inspecting a contract.

The following are deliberately **not implemented**, and no supporting code
exists in the Playground:

- Wallet connection
- Private-key or seed-phrase handling
- Transaction signing
- Transaction submission
- Contract deployment
- RPC calls
- Testnet or mainnet actions

Inspection therefore requires no network access beyond loading the page itself.

---

## Error Handling

Failures map to stable, user-facing messages. Internal parser wording, byte
offsets, and Rust panics or backtraces are never surfaced.

| Condition | `sdkt-wasm` variant | Behaviour |
|---|---|---|
| Empty file | `WasmError::Empty` | Reports that the file is empty and asks for a compiled `.wasm` contract |
| Not WebAssembly | `WasmError::Parse` | Reports that the file is not a valid WebAssembly module and may be corrupted or truncated |
| Malformed module | `WasmError::Parse` | Same message; results stay hidden |
| Valid module without a contract spec | `WasmError::NoContractSpec` | Metadata still renders; the specification section reports that no `contractspecv0` section is present |
| Spec section not decodable as XDR | `WasmError::SpecXdr` | Reports that the section could not be decoded, possibly built with an incompatible SDK |

A module carrying an empty `contractspecv0` section renders each specification
group with an explicit empty state rather than omitting it.

This mirrors the CLI, which also treats the contract specification as optional.

---

## Testing & Verification

| Area | Result |
|---|---|
| Rust formatting (`cargo fmt --all -- --check`) | Pass |
| Clippy (`--workspace --all-targets -- -D warnings`) | Pass |
| Workspace tests (`cargo test --workspace`) | Pass |
| Playground crate tests | 8/8 |
| `wasm32-unknown-unknown` release build | Pass |
| JavaScript syntax check | Pass |
| Browser E2E — valid contract | Pass |
| Browser E2E — invalid (non-WASM) input | Pass |
| Browser E2E — malformed module | Pass |
| Browser E2E — empty file | Pass |
| Browser E2E — reset and re-upload | Pass |
| Desktop 1280×900 | Pass |
| Mobile 390×844, no horizontal overflow | Pass |
| Console errors | 0 |
| External network requests during inspection | 0 |
| Live deployment | Pass |

The playground crate's tests live in
`crates/sdkt-playground/tests/inspect_test.rs` and run on the native target,
because `#[wasm_bindgen]` functions cannot be called from the native test runner.
They exercise the same code path the browser wrapper calls, plus this crate's
error-message mapping: a fixture yielding metadata and a parsed specification,
two distinct fixtures producing distinct hashes, empty input, non-WebAssembly
input, a malformed module, a valid module without a contract spec, deterministic
repeat inspection, and a 64 KiB custom section.

Run them with:

```
cargo test --manifest-path crates/sdkt-playground/Cargo.toml
```

Browser end-to-end coverage uses Playwright against Chromium with
`crates/sdkt-cli/tests/fixtures/us_old.wasm`, asserting on the rendered DOM
rather than on screenshots. Because the committed fixtures only declare
single-parameter functions, rendering is also checked against a synthetic module
carrying a four-parameter documented function with a return type, a three-member
struct, a three-variant enum, and a documented event; `sdkt wasm inspect` serves
as the oracle for such fixtures.

### CLI parity

For both committed fixtures, `sdkt wasm inspect` produces byte-identical
human-readable and `--format json` output before and after the Playground was
added, so introducing the Playground did not alter CLI behaviour.

For the same input, the Playground reports the values the CLI reports: identical
SHA-256, byte size, module version, custom sections, function names and
parameters, custom types, and events.

---

## Production Status

The Playground is deployed to GitHub Pages:

**https://SaboLabs.github.io/soroban-devkit/playground/**

The Pages workflow (`.github/workflows/pages.yml`) uploads `website/` verbatim on
pushes that touch `website/**` or the workflow itself. Because it performs no
build step, deploying is a matter of pushing to `main`.

### Regenerating the browser artifacts

The generated `wasm-bindgen` output is committed under `website/playground/wasm/`
(`sdkt_playground_bg.wasm`, 115,936 bytes; `sdkt_playground.js`, 10,687 bytes).
Since Pages does not build anything, **these artifacts must be regenerated and
committed whenever `sdkt-wasm` or `sdkt-playground` changes**, or the deployed
runtime will lag the Rust source.

Prerequisites:

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.127 --locked
```

Then:

```
bash website/playground/build.sh
```

The build is deterministic: rebuilding unchanged sources reproduces identical
artifact digests, so comparing checksums before and after a rebuild is a reliable
staleness check.

---

## CLI Relationship

The `sdkt` CLI remains the primary developer interface and the source of truth.
It exposes the full command set, `--format json` output, and CI integration.

The Playground is an additional browser interface for a selected subset of that
functionality — currently WASM inspection only. It does not offer parity with
the CLI and is not intended to.

Two rules keep the two front ends consistent:

1. Parsing is never reimplemented in JavaScript. All inspection goes through
   `sdkt-wasm` compiled to `wasm32-unknown-unknown`.
2. `sdkt-playground` stays a thin binding layer with no inspection logic, so
   behaviour cannot drift between the CLI and the browser.

---

## Limitations / Future Work

Current limitations:

- Only the Contract Inspector is functional. The Analyze, XDR, Diff, and Health
  navigation entries are placeholders.
- No wallet, signing, transaction submission, deployment, or RPC functionality,
  by design.
- Browser testing is Chromium-only. The Web Worker is an ES module worker, which
  older Firefox and Safari versions do not support, and no fallback is provided.
- CI does not cover the Playground. `ci.yml` builds and tests the native
  workspace, which excludes `crates/sdkt-playground`, so neither the
  `wasm32-unknown-unknown` build nor the playground crate's tests run in CI.
- `crates/sdkt-playground/Cargo.toml` hardcodes its version because the crate
  sits outside the workspace and cannot inherit `[workspace.package]`. It must be
  bumped manually on release.
- Struct and enum members display names only. `sdkt-wasm`'s `TypeMember` carries
  a name and doc comment but no type, so member types cannot be shown without
  extending the inspection logic.
- Presentation details still open: the specification column is not width-capped on
  wide viewports, entries with no parameters or members still render a card
  border, and the specification group count format differs from the badge style
  used by the other result sections.

Possible future work, none of it scheduled: extending CI to build the playground
crate for `wasm32-unknown-unknown` and run its tests, widening the browser matrix
beyond Chromium, and building out the remaining navigation surfaces from existing
CLI functionality.

---

## Release History

| Commit | Change |
|---|---|
| `46c02a2` | Web Playground MVP — the `sdkt-playground` binding crate, the Inspector page, worker and build script, committed browser artifacts, and landing-page entry points. Also removed an unused `sdkt-core` dependency from `sdkt-wasm`, which had blocked compiling that crate for `wasm32-unknown-unknown`; recorded in `CHANGELOG.md` as a non-breaking change. |
| `0a7e5bc` | Improved contract specification rendering. Replaced inline chips built from concatenated markup with semantic elements: an `<article>` per entry, a `<dl>` of parameter name/type rows, a separated and de-emphasised return-type row, and a distinct badge for custom-type kinds. Frontend only; the Rust inspector, its data structures, the `wasm-bindgen` API, and the worker architecture were unchanged. |
