# Soroban DevKit Web Playground — Project Checkpoint

**Checkpoint created:** 2026-08-12
**Repository:** `/home/ubuntu/soroban-devkit`
**Branch:** `main` (tracking `origin/main`)
**HEAD:** `924d93824e139650c7faccdc4a703abbb113b1b8` — `refactor(website): adopt developer tooling visual design`
**Workspace version:** `2.5.0`

This document records the **verified** state of the Web Playground work. Every
claim was checked against the filesystem, `git`, or real command output. Items
that could not be verified are marked explicitly as unverified.

---

## 1. Project Goal

Build a **browser-based Web Playground** exposing a selected subset of Soroban
DevKit functionality — starting with contract WASM inspection — so a developer
can evaluate the toolkit without installing anything.

The **Rust CLI (`sdkt`) remains the source of truth and the primary developer
interface.** The Playground is a deliberately narrower convenience surface:

- The CLI exposes the full command set, `--format json`, and CI integration.
- The Playground reuses the **same Rust inspection functions**, compiled to
  `wasm32-unknown-unknown`, so parsing behaviour cannot diverge from the CLI.
- No JavaScript reimplementation of any parsing logic.

---

## 2. Current Status

| Item | Status |
|---|---|
| Landing page (`website/index.html`) | Committed at HEAD; deployed via GitHub Pages workflow (`.github/workflows/pages.yml`). Working tree adds 2 uncommitted lines linking to the Playground. |
| Web Playground (`website/playground/`) | Implemented, **untracked** (never committed). |
| Playground glue crate (`crates/sdkt-playground/`) | Implemented, **untracked** (never committed). |
| Playground deployed? | **No.** Files are untracked, so the Pages workflow has never seen them. Nothing Playground-related is live. |
| Implementation phase | Phase 1 (Contract Inspector MVP) — code complete, **browser-validated locally**, not committed, not deployed. |
| Pre-release validation | **Executed 2026-08-12.** Rust gates green; browser E2E green on desktop 1280px and mobile 390px; local-only processing confirmed against observed network traffic. |
| Release decisions | **Both resolved 2026-08-12** — see §13. Pages stays build-free with committed artifacts; `sdkt-core` removal documented as a non-breaking `Unreleased` / `Changed` CHANGELOG entry. |
| CLI regression | **No regression.** `sdkt wasm inspect` output is byte-identical to the pre-Playground HEAD build — see §13.4. |

**Complete (locally, verified):**

- `crates/sdkt-playground` — wasm-bindgen glue crate exposing `inspect_wasm` and `sdkt_version`.
- 8 native integration tests in `crates/sdkt-playground/tests/inspect_test.rs` — all passing.
- `website/playground/` — `index.html`, `playground.css`, `playground.js`, `worker.js`, `build.sh`.
- Generated browser artifacts committed to the working tree (untracked):
  `website/playground/wasm/sdkt_playground.js` (11K) and
  `website/playground/wasm/sdkt_playground_bg.wasm` (114K).
- `wasm32-unknown-unknown` release build succeeds (`cargo build -p sdkt-playground --release --target wasm32-unknown-unknown`).
- Landing-page CTA + nav link to `playground/` (uncommitted edit to `website/index.html`).

**Pending (non-blocking):**

- Three cosmetic CSS defects found during 390px review (see §10 items 8–10).
  Deliberately **not** fixed — explicitly deferred by the operator.
- CI coverage for the Playground crate (see §10 item 7).
- Broader browser matrix — Firefox / Safari (see §10 item 11).

**Pending (gated on operator approval):**

- Commit + push.
- Pages deployment of the Playground.

**Resolved:**

- Browser E2E validation — **executed**, results in §9c and re-run in §13.5.
- Mobile 390px viewport — **executed**, no horizontal overflow.
- Console error check — **executed**, zero messages, zero page errors.
- `playground.js` `file.type` fallback precedence bug — **fixed**.
- `.gitignore` coverage for `crates/sdkt-playground/target/` — **verified already covered**.
- Staleness of `website/playground/wasm/` artifacts — **verified current** (byte-identical rebuild, twice).
- GitHub Pages artifact strategy — **decided: build-free + committed artifacts** (§13.2).
- `sdkt-core` removal release treatment — **decided: `Unreleased` / `Changed` CHANGELOG entry, non-breaking** (§13.1).
- Version consistency across workspace / crates / website — **verified aligned at 2.5.0** (§13.3).
- CLI regression check — **byte-identical output vs pre-Playground HEAD** (§13.4).

---

## 3. Phase 0 Architecture Research

Verified findings from reading the actual source:

**Existing inspection implementation lives in `crates/sdkt-wasm`:**

- `sdkt_wasm::parse_metadata(&[u8]) -> Result<WasmMetadata, WasmError>`
  (`crates/sdkt-wasm/src/lib.rs:65`) — walks the module with
  `wasmparser::Parser`, computes SHA-256 via `sha2`, collects exports, imports,
  and custom-section names.
- `sdkt_wasm::parse_contract_spec(&[u8]) -> Result<ContractSpec, WasmError>`
  (`crates/sdkt-wasm/src/spec.rs:110`) — same `wasmparser` walk, decodes the
  `contractspecv0` custom section as a sequence of XDR `ScSpecEntry` values and
  the `contractenvmetav0` section as a `u64` interface version.

**Error type — `sdkt_wasm::WasmError` (`crates/sdkt-wasm/src/lib.rs:19-29`), exactly four variants:**

| Variant | Meaning |
|---|---|
| `Parse(wasmparser::BinaryReaderError)` | malformed / non-WASM input |
| `Empty` | zero-length input |
| `NoContractSpec` | valid WASM, no `contractspecv0` section |
| `SpecXdr(stellar_xdr::Error)` | spec section present but not decodable as XDR |

**Why the existing Rust logic is reusable as-is:**

- Both functions are pure: `&[u8]` in, owned struct out. No filesystem, no
  network, no global state.
- All returned types (`WasmMetadata`, `ContractSpec`, and children) already
  derive `Serialize`/`Deserialize`, so they cross the JS boundary via
  `serde-wasm-bindgen` without new conversion code.
- The CLI itself (`crates/sdkt-cli/src/main.rs:2768-2774`) calls exactly these
  two functions, treating the spec as optional (`parse_contract_spec(..).ok()`).
  The Playground mirrors that contract.

**wasm32 / browser compatibility findings:**

- `sdkt-wasm`'s dependency set (`wasmparser`, `sha2`, `serde`, `stellar-xdr`,
  `hex`) compiles for `wasm32-unknown-unknown`.
- **Blocker found and resolved:** `crates/sdkt-wasm/Cargo.toml` declared
  `sdkt-core` as a dependency, which transitively pulls `zstd`/`tar` (C sources)
  and fails to build for `wasm32-unknown-unknown`. Verified by grep that
  `sdkt-wasm/src/` contains **no** reference to `sdkt_core` — the dependency was
  unused. It was removed (working-tree change, uncommitted).
- `crates/sdkt-playground` is **excluded** from the root workspace
  (`Cargo.toml` `exclude = ["crates/sdkt-playground"]`) so that
  `cargo build/clippy/test --workspace` stays a pure native build with no
  wasm-bindgen in the graph. It therefore declares its own `[workspace]` root
  and cannot use `field.workspace = true` inheritance.

**Core rewrite required?** No. Zero changes to inspection logic. The only Rust
change outside the new crate is the removal of the unused `sdkt-core`
dependency from `sdkt-wasm`.

---

## 4. Existing Inspector Behavior

What `sdkt wasm inspect <file.wasm>` currently extracts, taken from
`WasmMetadata` / `ContractSpec` field definitions and the CLI print path
(`crates/sdkt-cli/src/main.rs:2760-2815`). No invented fields.

**From `parse_metadata` → `WasmMetadata`:**

| Field | Type | Notes |
|---|---|---|
| `hash` | `String` | SHA-256 of the raw bytes, hex-encoded |
| `size_bytes` | `usize` | length of input bytes |
| `version` | `u16` | WASM module version from `Payload::Version` (defaults to `1`) |
| `exports` | `Vec<WasmExport>` | each `{ name: String, kind: String }` |
| `imports` | `Vec<WasmImport>` | each `{ module: String, name: String, kind: String }` |
| `custom_sections` | `Vec<String>` | custom-section names only, not payloads |

**From `parse_contract_spec` → `ContractSpec` (optional):**

| Field | Type | Notes |
|---|---|---|
| `env_meta` | `Option<EnvMetaSpec>` | `{ interface_version: u64 }` from `contractenvmetav0` |
| `functions` | `Vec<ContractFunction>` | `{ name, doc, parameters, outputs }`; parameters are `{ name, doc, type_ }` |
| `custom_types` | `Vec<ContractType>` | `{ name, kind, doc, members }`; `kind` ∈ `struct`, `union`, `enum`, `error_enum` (plus `primitive`/`compound`/`udt` for type refs) |
| `events` | `Vec<ContractEvent>` | `{ name, doc }` |

**CLI pretty output prints:** size, SHA-256 hash, version, custom sections list,
exported functions list, then `Contract Spec Available: Yes/No` with function
count, per-function arity, custom-type count, and event count. `--format json`
emits `{ file, metadata, spec }`.

**Not extracted anywhere:** custom-section payload bytes, function bodies,
per-function bytecode size, source maps.

---

## 5. Planned Playground Architecture

```
sdkt-wasm                (parse_metadata, parse_contract_spec — unchanged)
    |
    +-- CLI              (crates/sdkt-cli — source of truth)
    |
    +-- sdkt-playground  (crates/sdkt-playground — thin glue, no logic)
            |
            +-- wasm-bindgen   (--target web, --no-typescript)
                    |
                    +-- Web Worker    (website/playground/worker.js, ES module)
                            |
                            +-- Browser UI  (index.html + playground.js + playground.css)
```

**Implemented data flow (verified in code):**

1. `playground.js` reads the picked/dropped file with `file.arrayBuffer()` into a
   `Uint8Array` on the main thread.
2. Bytes are handed to the worker via `postMessage`.
3. `worker.js` imports the wasm-bindgen shim (`./wasm/sdkt_playground.js`),
   boots it against `./wasm/sdkt_playground_bg.wasm`, and calls
   `inspect_wasm(bytes)`.
4. `inspect_wasm` calls `parse_metadata` (mandatory) and `parse_contract_spec`
   (optional), returning `{ metadata, spec, spec_error }` serialized by
   `serde-wasm-bindgen`.
5. The worker adds `duration_ms` (measured with `performance.now()`) and posts
   the result back; `playground.js` renders it into DOM sections.

**Locality:** uploaded WASM stays in the browser. The bytes live only as a
`Uint8Array` on the page and inside the worker's WebAssembly instance. The only
network requests the app makes are for its own static assets (HTML, CSS, worker
script, and the bundled `.wasm` runtime). There is no upload endpoint, no RPC
call, and no telemetry in the Playground code.

---

## 6. MVP Scope

**IN SCOPE (implemented):**

- WASM file upload — drag-and-drop zone plus file picker (`accept=".wasm,application/wasm"`).
- Local inspection, entirely in-browser.
- Actual Rust inspection logic (`sdkt_wasm::parse_metadata` + `parse_contract_spec`) via wasm-bindgen — no JS heuristics.
- Structured results: Metadata, Exports, Imports, Custom Sections, Contract Specification (functions / custom types / events).
- Error handling — mapped user-facing messages, no Rust panics or stack traces surfaced.
- Reset / re-upload button.
- Responsive UI (dark theme, `prefers-reduced-motion` honoured, skip link, ARIA live regions, keyboard-accessible dropzone).
- CLI link ("Use sdkt CLI" → `../#install`).
- Landing-page CTA ("Open Web Playground") + nav entry.

**OUT OF SCOPE (absent from the code — verified by inspection):**

- Wallet connection
- Private keys
- Seed phrases / mnemonics
- Transaction signing
- Transaction submission
- Contract deployment
- RPC calls
- Mainnet actions
- Testnet actions

`playground.js` contains a `networkProvider = { mode: 'local', supported: ['local'] }`
placeholder object. It is inert — no code path reads it to perform network work.

---

## 7. Files Already Changed

Verified with `git status --short --branch` and `git log --all -- <path>`.

| File | Exists | Committed | Pushed | Deployed |
|---|---|---|---|---|
| `crates/sdkt-playground/Cargo.toml` | yes | no (untracked) | no | n/a |
| `crates/sdkt-playground/Cargo.lock` | yes | no (untracked) | no | n/a |
| `crates/sdkt-playground/src/lib.rs` | yes | no (untracked) | no | n/a |
| `crates/sdkt-playground/tests/inspect_test.rs` | yes | no (untracked) | no | n/a |
| `crates/sdkt-playground/target/` | yes | no (untracked build output) | no | n/a |
| `website/playground/index.html` | yes | no (untracked) | no | no |
| `website/playground/playground.css` | yes | no (untracked) | no | no |
| `website/playground/playground.js` | yes (fix applied this round, line 311) | no (untracked) | no | no |
| `website/playground/worker.js` | yes | no (untracked) | no | no |
| `website/playground/build.sh` | yes | no (untracked) | no | no |
| `website/playground/wasm/sdkt_playground.js` | yes (11K) | no (untracked) | no | no |
| `website/playground/wasm/sdkt_playground_bg.wasm` | yes (114K) | no (untracked) | no | no |
| `Cargo.toml` (root) | yes | modified, uncommitted | no | n/a |
| `Cargo.lock` (root) | yes | modified, uncommitted | no | n/a |
| `crates/sdkt-wasm/Cargo.toml` | yes | modified, uncommitted | no | n/a |
| `website/index.html` | yes | modified, uncommitted (2 added lines) | no | HEAD version is deployed; the Playground links are not |

**Content of the tracked-file modifications:**

- `Cargo.toml` — added `exclude = ["crates/sdkt-playground"]` plus explanatory comment.
- `crates/sdkt-wasm/Cargo.toml` — removed the unused `sdkt-core = { version = "2.4.0", path = "../sdkt-core" }` dependency, replaced with a comment explaining the wasm32 rationale.
- `Cargo.lock` — one line removed: `sdkt-core` from `sdkt-wasm`'s dependency list.
- `website/index.html` — two added lines: nav link `<a href="playground/">Playground</a>` (line 32) and `<a class="btn btn-ghost" href="playground/">Open Web Playground</a>` (line 50).

**Note:** `crates/sdkt-playground/target/` is untracked build output living
inside the untracked crate directory. It should not be committed; the crate is
outside the root workspace so its artifacts land there rather than in the root
`target/`. A `.gitignore` entry may be needed before staging (open question).

---

## 8. Git State

```
branch:  main  (## main...origin/main — no ahead/behind divergence reported)
HEAD:    924d93824e139650c7faccdc4a703abbb113b1b8
subject: refactor(website): adopt developer tooling visual design
```

**Working tree (`git status --short --branch`) as of the validation round:**

```
## main...origin/main
 M Cargo.lock
 M Cargo.toml
 M crates/sdkt-wasm/Cargo.toml
 M website/index.html
?? PLAYGROUND_PROGRESS.md
?? crates/sdkt-playground/
?? website/playground/
```

`git diff --check` → clean (exit 0, no whitespace errors).

Note: `website/playground/playground.js` carries this round's fix but shows no
`M` marker because the whole `website/playground/` directory is still untracked.
Likewise `PLAYGROUND_PROGRESS.md` is untracked. No `.gitignore` change was
needed — `**/target/` at `.gitignore:4` already covers
`crates/sdkt-playground/target/`.

**Latest relevant commits:**

```
924d938 refactor(website): adopt developer tooling visual design
84e3ec8 fix(website): clarify verified CLI workflow
10076b2 ci(website): deploy landing page via GitHub Pages
13c37a3 feat(website): add Soroban DevKit landing page
dcf2cd9 docs(roadmap): sync milestone status through M44
```

`git log --all -- website/playground/ crates/sdkt-playground/` returns **no**
commits touching those paths. All Playground work is uncommitted.

**Uncommitted?** Yes — 4 modified tracked files, 2 untracked directories, plus
this checkpoint file (`PLAYGROUND_PROGRESS.md`, untracked).

---

## 9. Tests Already Performed

Only actually-executed commands and their real results are listed.

| Check | Command | Result |
|---|---|---|
| Workspace format | `cargo fmt --all -- --check` | **PASS** (no diff, exit 0) |
| Playground crate format | `cargo fmt --manifest-path crates/sdkt-playground/Cargo.toml -- --check` | **PASS** (no diff, exit 0) |
| Workspace clippy | `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** (exit 0, no warnings) |
| Playground clippy | `cargo clippy --manifest-path crates/sdkt-playground/Cargo.toml --all-targets -- -D warnings` | **PASS** (exit 0, no warnings) |
| Workspace tests | `cargo test --workspace` | **PASS** (exit 0; all suites green, including doc-tests for `sdkt_wasm`, `sdkt_rpc`, `sdkt_storage`, `sdkt_xdr`) |
| Playground tests | `cargo test --manifest-path crates/sdkt-playground/Cargo.toml` | **PASS** — 8/8 in `tests/inspect_test.rs`, 0 failed |
| wasm32 build | `cargo build -p sdkt-playground --release --target wasm32-unknown-unknown --manifest-path crates/sdkt-playground/Cargo.toml` | **PASS** (exit 0; artifact at `crates/sdkt-playground/target/wasm32-unknown-unknown/release/sdkt_playground.wasm`, 447K pre-bindgen) |
| Toolchain availability | `rustup target list --installed`, `wasm-bindgen --version` | `wasm32-unknown-unknown` installed; `wasm-bindgen 0.2.127` installed (matches the version pinned in `build.sh` and `Cargo.toml`) |

**The 8 passing playground tests:**

1. `valid_fixture_yields_metadata_and_spec` — real fixture `us_old.wasm`: 64-char SHA-256, version 1, `contractspecv0` present, 2 functions (`transfer`, `mint`), 1 event, 1 custom type.
2. `second_fixture_differs_from_first` — `us_old.wasm` vs `us_new.wasm` hash inequality and function-count growth.
3. `empty_input_is_rejected_with_friendly_message`
4. `non_wasm_bytes_are_rejected`
5. `malformed_wasm_is_rejected` — asserts `BinaryReaderError` does not leak into the message.
6. `valid_module_without_contract_spec_still_inspects`
7. `repeated_inspection_is_deterministic`
8. `larger_input_does_not_break_parsing` — 64 KiB custom section.

Fixtures used are the existing CLI fixtures
`crates/sdkt-cli/tests/fixtures/us_old.wasm` and `us_new.wasm`.

### 9b. Pre-Release Validation Round — 2026-08-12

All Rust gates re-run after the `playground.js` fix. Results:

| Check | Command | Result |
|---|---|---|
| Workspace format | `cargo fmt --all -- --check` | **PASS** (exit 0) |
| Workspace clippy | `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** (exit 0) |
| Workspace tests | `cargo test --workspace` | **PASS** — 112 + 46 + 31 + 25 + 23 + 3 + 2 unit/integration passing, 0 failed; all 4 doc-test suites pass |
| Playground tests | `cargo test --manifest-path crates/sdkt-playground/Cargo.toml` | **PASS** — 8/8, 0 failed |
| wasm32 build | `cargo build -p sdkt-playground --release --target wasm32-unknown-unknown` | **PASS** (exit 0) |

**Artifact freshness (verified, §4 of the task):** `website/playground/build.sh`
was re-run after touching `crates/sdkt-playground/src/lib.rs` to force a
recompile. SHA-256 before and after are identical, so the checked-in artifacts
were **already generated from current source** — not stale:

```
sdkt_playground.js      2d2bf607fbc47ccbb2b886a53c1ec4e4804b8696c74a17c0787ac5565d4a662c  (unchanged)
sdkt_playground_bg.wasm a54984967bf1d7b8404035b58196578e00ae98f65f33346f86ddf41b8d375c77  (unchanged)
```

**`sdkt-core` removal re-verification (§3 of the task — removal NOT undone):**

- `grep -rn "sdkt_core\|sdkt-core" crates/sdkt-wasm/src/` → **no matches**. The
  crate genuinely does not use it.
- Native workspace tests still pass without it (row above).
- wasm32 playground build still passes (row above).
- Conclusion: the removal is correct and safe on both targets. Release
  bookkeeping remains open — see §10 item 1.

**`.gitignore` (§2 of the task):** no new rule was needed.
`git check-ignore -v crates/sdkt-playground/target/` resolves to
`.gitignore:4:**/target/`, which already covers it. Confirmed with
`git add -An crates/sdkt-playground/`, which stages only `Cargo.toml`,
`Cargo.lock`, `src/lib.rs`, and `tests/inspect_test.rs` — no build output.

**`playground.js` fix (§1 of the task):** line 311 changed from
`fmtSize(file.size) + ' · ' + file.type || 'application/octet-stream'` to
`fmtSize(file.size) + ' · ' + (file.type || 'application/octet-stream')`.
Verified in the browser: a `File` constructed with `type: ''` now renders
`198 B · application/octet-stream`; with `type: 'application/wasm'` it renders
`198 B · application/wasm`. Before the fix the fallback was dead code.

### 9c. Browser E2E — EXECUTED

Served with `python3 -m http.server 8099 --bind 127.0.0.1` from `website/`.
Driven by Playwright Chromium (`/tmp/pg_e2e.py`). Every assertion below comes
from real captured output, not inference.

| Scenario | Observed | Verdict |
|---|---|---|
| Page + WASM runtime load | `#modeChip` = "Local analysis — no RPC required"; error box hidden; worker booted | **PASS** |
| Static asset serving | `/playground/` 200, `playground.css` 200, `playground.js` 200, `worker.js` 200, `wasm/sdkt_playground.js` 200, `wasm/sdkt_playground_bg.wasm` 200 `Content-Type: application/wasm` | **PASS** |
| Valid fixture (`us_old.wasm`, 198 B) | status `inspection complete · 5.3 ms`; SHA-256 `05befa136e7f0829a5051d97b032f355a5e65976397df90b224d141942dce46c` (matches `sha256sum` of the file on disk); Size `198 B (198 bytes)`; Version 1; Custom Sections 1 = `contractspecv0`; Functions (2) `fn transfer(to: address)`, `fn mint(amt: u32)`; Custom Types (1) `type Point struct`; Events (1) `event Transfer` | **PASS** |
| Structured rendering | 5 sections present: Metadata, Exports (0 items), Imports (0 items), Custom Sections (1 item), Contract Specification (available) | **PASS** |
| Non-WASM input (plain text) | status `inspection failed`; message "This file is not a valid WebAssembly module. It may be corrupted, truncated, or not a .wasm file at all."; results hidden; no stack trace, no `BinaryReaderError` | **PASS** |
| Malformed WASM (magic + garbage section header) | same safe message; results hidden | **PASS** |
| Empty file (0 bytes) | status `inspection failed`; message "The file is empty. Select a compiled .wasm contract." | **PASS** |
| Reset | filebar hidden, results hidden, error cleared, `fileInput.value === ''` | **PASS** |
| Re-upload after reset | status `inspection complete · 0.2 ms`, results re-rendered with the same 2 functions | **PASS** |
| Console (desktop 1280×900) | 0 console messages, 0 page errors | **PASS** |
| Console (mobile 390×844) | 0 console messages, 0 page errors | **PASS** |
| Mobile 390px inspection | status `inspection complete · 2.9 ms`, results rendered | **PASS** |
| Horizontal overflow at 390px | `documentElement.clientWidth` 390, `scrollWidth` 390, `body.scrollWidth` 390 → **no** horizontal overflow. The only element outside the viewport is `a.skip` at `left: -9999px`, which is the intentional off-screen skip-link pattern. | **PASS** |

**Security — contract bytes stay in the browser (verified, not assumed):**

Total requests observed across the full session (load + valid inspection +
3 failure cases + reset + re-upload): **6**, all `GET`, all to
`127.0.0.1:8099/playground/*`:

```
/playground/
/playground/playground.css
/playground/playground.js
/playground/worker.js
/playground/wasm/sdkt_playground.js
/playground/wasm/sdkt_playground_bg.wasm
```

- Non-GET or body-carrying requests: **0**
- External-origin requests: **0**
- Fixture bytes appearing in any request URL: **no**
- Separately, `fetch`, `XMLHttpRequest.send`, `navigator.sendBeacon`, and
  `WebSocket` were monkey-patched in-page before running an inspection; the
  recorded call log was **empty** and no new resource entries appeared during
  inspection.

Conclusion: inspection triggers **zero** network activity. The only traffic is
the Playground's own static assets. **PASS**

Also confirmed by absence in the source: no wallet, no key handling, no signing,
no RPC, no deployment, no testnet/mainnet code paths were added.

---

## 10. Known Open Questions

1. ~~`sdkt-core` dependency removal — needs a decision.~~ **RESOLVED** — see
   §13.1. Documented as a non-breaking `Unreleased` / `Changed` CHANGELOG entry.
   The dependency was **not** re-added.

2. ~~Generated `wasm/` artifacts — commit or build in CI?~~ **RESOLVED** — see
   §13.2. Decision: keep `pages.yml` build-free and commit the artifacts.

3. ~~`crates/sdkt-playground/target/` must not be committed.~~ **RESOLVED** —
   already covered by `.gitignore:4` (`**/target/`). Verified twice with
   `git check-ignore` and `git add -An`; no new rule was needed.

4. **Playground crate version sync.** `crates/sdkt-playground/Cargo.toml`
   hardcodes `version = "2.5.0"` because the crate is outside the root
   workspace and cannot inherit `[workspace.package]`. This must be bumped
   manually on every release. Currently correct; no automation exists.

5. **Browser compatibility — unverified.** The worker is an ES module worker
   (`new Worker('worker.js', { type: 'module' })`). Module workers are
   unsupported in older Safari and Firefox versions. No fallback exists and no
   browser matrix was tested.

6. ~~`playground.js:311` operator-precedence bug.~~ **FIXED** in this round;
   see §9b. Wrapped in parentheses so the fallback applies; verified in-browser.

7. **CI does not cover the Playground.** `ci.yml` runs the native workspace,
   which now excludes `crates/sdkt-playground`. Neither the wasm32 build nor the
   8 playground tests run in CI. Needs a decision on adding a job.

8. **Spec-row badge/label spacing at 390px (cosmetic, measured).** In
   `#results` the spec rows use `display: inline-flex` with `gap: normal`, so
   the measured gap between the kind badge (`.k`) and the following identifier
   is **0px** — rendering as `fn transfer(...)` with the badge visually touching
   the name. Measured values: `name: contractspecv0` chip 8px (correct, it is a
   `.kv-row .kv` with `gap: 8px`), but the four spec rows built inline in
   `playground.js` (`fn`, `type`, `event`) all report `measuredGapPx: 0`.
   Cause: those rows are created with inline styles and never get the
   `.kv-row` parent that supplies the gap. Not fixed — cosmetic, and fixing it
   means touching CSS/JS beyond the scope authorised for this round.

9. **Size cell word-breaks mid-token (cosmetic).** `.r-table td` sets
   `word-break: break-all` globally, so at 390px `198 B (198 bytes)` can break
   inside `bytes`. The rule is needed for the 64-char SHA-256 but is too broad;
   `overflow-wrap: anywhere` scoped to the hash cell would be the correct fix.
   Not fixed.

10. **Privacy-note icon renders as tofu.** `website/playground/index.html:54`
    uses a literal `🔒` emoji; on a system without an emoji font it renders as
    an empty box. Cosmetic. Not fixed.

11. **Browser matrix still narrow.** E2E ran on **Chromium only**. The ES-module
    worker (`new Worker(..., { type: 'module' })`) remains unverified on Firefox
    and Safari. Item 5 above stands.

---

## 11. Next Exact Step

**Stage and commit the validated Playground work. Nothing else is blocking.**

Exact staging set (verified with `git add -An` — no build output included):

```
git add PLAYGROUND_PROGRESS.md CHANGELOG.md Cargo.toml Cargo.lock \
        crates/sdkt-wasm/Cargo.toml \
        crates/sdkt-playground/Cargo.toml crates/sdkt-playground/Cargo.lock \
        crates/sdkt-playground/src/lib.rs crates/sdkt-playground/tests/inspect_test.rs \
        website/index.html \
        website/playground/index.html website/playground/playground.css \
        website/playground/playground.js website/playground/worker.js \
        website/playground/build.sh \
        website/playground/wasm/sdkt_playground.js \
        website/playground/wasm/sdkt_playground_bg.wasm
```

`crates/sdkt-playground/target/` is excluded automatically by `.gitignore:4`.

Deployment happens on push (the Pages workflow triggers on `website/**`), so
**do not push until deployment is explicitly approved.**

Both release decisions are resolved (§13). All Rust gates, the CLI regression
check, and the browser E2E suite are green against the exact artifacts that
would be committed.

---

## 12. Critical Rules

These constraints are binding for all further Playground work:

1. **The CLI remains the source of truth.** The Playground never becomes the
   primary interface.
2. **Do not fake Rust inspection with JavaScript heuristics.** All parsing goes
   through `sdkt_wasm` compiled to wasm32.
3. **Reuse existing Rust logic.** `crates/sdkt-playground` stays a thin glue
   layer with no inspection logic of its own.
4. **Do not modify CLI behavior.** No changes to `sdkt-cli` output, flags, or
   semantics for the Playground's benefit.
5. **Process uploaded WASM locally.** Bytes never leave the browser.
6. **No private keys.**
7. **No wallet.**
8. **No transaction signing.**
9. **No deployment** (contract deployment).
10. **No RPC.**
11. **Do not deploy the Playground until explicitly approved.**

---

## 13. Release-Readiness Round — 2026-08-12

This section records the release decisions and the verification behind them.
Every result below is real command output.

### 13.1 `sdkt-wasm` manifest change — decision

**Verification (all four checks required by the task):**

| Check | Method | Result |
|---|---|---|
| `sdkt_core` genuinely unused | `grep -rn "sdkt_core\|sdkt-core" crates/sdkt-wasm/ --include=*.rs --include=*.toml` | Only two hits, both inside the explanatory **comment** in `Cargo.toml`. Zero hits in `src/`. |
| Native build/tests green | `cargo test --workspace` | **PASS** — 112 + 46 + 31 + 25 + 23 + 3 + 2, 0 failed; 4 doc-test suites pass |
| wasm32 playground build green | `cargo build -p sdkt-playground --release --target wasm32-unknown-unknown` | **PASS** |
| CLI `sdkt wasm inspect` unchanged | byte-diff vs a `git worktree` build of HEAD `924d938` | **IDENTICAL** — see §13.4 |

**Decision — smallest correct treatment: a CHANGELOG entry, nothing else.**

Reasoning:

- `sdkt-wasm` is published on crates.io, so a manifest change is publicly
  visible and belongs in the changelog.
- It is **not** a breaking change. No `sdkt-core` type appears anywhere in
  `sdkt-wasm`'s public API (`WasmMetadata`, `WasmExport`, `WasmImport`,
  `WasmError`, `ContractSpec` and children are all defined locally or built from
  `serde`/`stellar_xdr` types). Downstream code cannot have been relying on the
  re-export, because there was no re-export — the dependency was simply declared
  and never referenced.
- Therefore: no version bump, no breaking-change note, no migration guidance.
- The dependency was **not** re-added. Gating it with
  `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` was considered and
  rejected: it would preserve a dependency that is unused on *every* target,
  which is strictly worse than removing it.

**Change applied:** one `### Changed` bullet under `## [Unreleased]` in
`CHANGELOG.md`, stating that the dependency was declared-but-unused, that removal
is non-breaking, that it unblocks `wasm32-unknown-unknown`, and that CLI output
is byte-identical.

### 13.2 GitHub Pages artifact strategy — decision

**Decision: keep `pages.yml` build-free and commit the generated artifacts.**
No Rust or `wasm-bindgen` steps were added to GitHub Actions. `pages.yml` is
**unmodified** (`git status` shows no change to `.github/`).

Acceptance checks on the artifact, all executed:

| Check | Result |
|---|---|
| Official build re-run | `bash website/playground/build.sh` → exit 0, `wasm-bindgen 0.2.127` |
| Artifact corresponds to current source | SHA-256 unchanged across a forced recompile: `sdkt_playground.js` `2d2bf607…5d4a662c`, `sdkt_playground_bg.wasm` `a5498496…8d375c77`. Same digests as the earlier round → deterministic and current. |
| No `target/` build output in git | `git add -An website/playground/ crates/sdkt-playground/` lists exactly 11 paths, none under `target/`. `git check-ignore -v crates/sdkt-playground/target/` → `.gitignore:4:**/target/`. |
| Artifact size reasonable | `sdkt_playground_bg.wasm` 115,936 B (113 KiB); `sdkt_playground.js` 10,687 B (10 KiB); directory total 132 KB. Acceptable for a static asset; smaller than a typical web font. |
| Browser E2E against the committed artifact | **PASS** — full suite re-run, §13.5 |

Trade-off accepted: the artifacts must be regenerated with
`bash website/playground/build.sh` whenever `sdkt-wasm` or `sdkt-playground`
changes. This is now the documented release step.

### 13.3 Version consistency — verified

| Surface | Value | Source |
|---|---|---|
| Workspace | `2.5.0` | `Cargo.toml` `[workspace.package] version` |
| `sdkt-wasm` | `2.5.0` | inherits via `version.workspace = true` |
| `sdkt-playground` | `2.5.0` | hardcoded (crate is outside the workspace, cannot inherit) |
| CLI binary | `sdkt 2.5.0` | `./target/release/sdkt -V` |
| Landing page | `v2.5.0` | `website/index.html` lines 34, 54, 68, 367, 441 |
| Playground page | `v2.5.0` | `website/playground/index.html` lines 29, 93 |

All aligned at **2.5.0**. Playground kept at v2.5.0 as instructed. No version
values were changed in this round.

### 13.4 CLI regression test — no regression

A clean `git worktree` was created at HEAD `924d938` (pre-Playground, with
`sdkt-core` still declared) and `sdkt-cli` was built from it, then both binaries
were run against the same fixtures and diffed.

```
diff head_pretty.txt   cur_pretty.txt     → IDENTICAL
diff head_json.txt     cur_json.txt       → IDENTICAL   (us_old.wasm --format json)
diff head_json_new.txt cur_json_new.txt   → IDENTICAL   (us_new.wasm --format json)

sha256  4b14dc70ecc4adeed41f1b6b5c9ad84d37e2e9fdf1fb2af04e406f1a0d7877d4  head_pretty.txt
sha256  4b14dc70ecc4adeed41f1b6b5c9ad84d37e2e9fdf1fb2af04e406f1a0d7877d4  cur_pretty.txt
sha256  4a02ad1f2a481b79554726a88175e97343b1e6955ccdea1ee2d2c8c6bf884615  head_json.txt
sha256  4a02ad1f2a481b79554726a88175e97343b1e6955ccdea1ee2d2c8c6bf884615  cur_json.txt

both binaries report: sdkt 2.5.0
```

Actual current output of
`sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_old.wasm`:

```
WASM Inspection Report: crates/sdkt-cli/tests/fixtures/us_old.wasm
========================================
Size: 198 bytes
SHA-256 Hash: 05befa136e7f0829a5051d97b032f355a5e65976397df90b224d141942dce46c
Version: 1

Custom Sections (1):
  - contractspecv0

Exported Functions (0):

Contract Spec Available: Yes
  Functions: 2
    - fn transfer(1) -> 0
    - fn mint(1) -> 0
  Custom Types: 1
  Events: 1
```

**Cross-check against the Playground:** the browser reports the same SHA-256
(`05befa13…dce46c`), the same size (198 bytes), the same single custom section
(`contractspecv0`), the same 2 functions (`transfer`, `mint`), 1 custom type
(`Point`), and 1 event (`Transfer`). Same Rust functions, same results, two
front-ends. The worktree was removed after the comparison
(`git worktree list` shows only the main tree).

### 13.5 Final validation — all gates re-run

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **PASS** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** |
| `cargo test --workspace` | **PASS** — 0 failed |
| `cargo test -p sdkt-playground` (via `--manifest-path`) | **PASS** — 8/8 |
| `cargo build -p sdkt-playground --release --target wasm32-unknown-unknown` | **PASS** |
| `git diff --check` | clean (exit 0) |

Browser E2E re-run against the current committed-candidate artifact
(Playwright Chromium, `python3 -m http.server 8099` from `website/`):

| Scenario | Result |
|---|---|
| Valid WASM (`us_old.wasm`) | **PASS** — `inspection complete · 16.2 ms`; SHA-256, size, version, `contractspecv0`, 2 functions, 1 custom type, 1 event all correct |
| Invalid WASM (plain text) | **PASS** — safe message, results hidden |
| Malformed WASM | **PASS** — safe message, results hidden |
| Empty file | **PASS** — "The file is empty. Select a compiled .wasm contract." |
| Reset / re-upload | **PASS** — cleared then `inspection complete · 0.2 ms` |
| Zero network activity during inspection | **PASS** — 6 requests total, all GET, all `/playground/*` static assets; 0 non-GET, 0 body-carrying, 0 external-origin |
| Zero console errors | **PASS** — 0 messages / 0 page errors on desktop **and** mobile |
| Mobile 390px | **PASS** — `inspection complete · 3.1 ms`, results rendered |
| No horizontal overflow | **PASS** — `clientWidth` 390 = `scrollWidth` 390 = `body.scrollWidth` 390 |

### 13.6 Exact files changed in this round

| File | Change |
|---|---|
| `CHANGELOG.md` | **+10 lines.** One `### Changed` bullet under `## [Unreleased]` documenting the `sdkt-core` removal as non-breaking. Only new content; nothing removed or reworded. |
| `PLAYGROUND_PROGRESS.md` | Updated status table, resolved/pending lists, §10 items 1–3 marked resolved, §11 rewritten, this §13 added. |

No other file was touched this round. Specifically **unchanged**:
`.github/workflows/pages.yml`, all Rust source, `crates/sdkt-playground/Cargo.toml`,
`website/playground/*` (the artifacts were regenerated to byte-identical
content, so they are unmodified in substance), and all version numbers.

### 13.7 Confirmed absent

Re-verified that this round added **no** wallet, no private-key handling, no
seed phrases, no signing, no RPC, no deployment, no testnet, and no mainnet
functionality. Nothing was committed, nothing was pushed, nothing was deployed.
