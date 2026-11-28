# Web Playground User Guide — Inspect and Compare a Contract WASM in the Browser

The **Soroban DevKit Web Playground** inspects compiled Soroban `.wasm` contracts and checks upgrade ABI compatibility directly in your browser. An ES module Web Worker (`website/playground/worker.js`) runs the same Rust inspection and spec-diff logic (`sdkt_wasm::parse_metadata`, `parse_contract_spec`, and upgrade-safety diffing) used by the `sdkt` CLI (`v2.7.0`). Contract bytes stay in browser memory and are never uploaded to a server.

***

## Prerequisites

Choose either option:

1. **Hosted Web Playground (zero install):** Open [https://soroban-devkit.xyz/playground/](https://soroban-devkit.xyz/playground/) in a modern desktop or mobile browser with WebAssembly and ES module Web Worker support.
2. **Local checkout:** Serve the `website/` directory over HTTP from the repository root:

   ```bash
   python3 -m http.server --directory website 8000
   ```

   Then open `http://localhost:8000/playground/` in your browser.

   > **Why HTTP is required:** Opening `website/playground/index.html` directly via `file://` will fail because browsers block ES module Web Workers (`worker.js`) and `fetch()` requests for `./wasm/sdkt_playground_bg.wasm` and bundled example `.wasm` files on `file://` origins.

When the worker finishes initializing the WASM runtime, the top-right mode badge displays **`Local analysis — no RPC required`**.

***

## Inspector Flow

### 1. Load a Contract WASM

You can load a base `.wasm` contract using any of three controls in the top upload row:

* **Dropzone (`Drop .wasm here`):** Drag and drop a compiled `.wasm` file onto the dashed dropzone, or focus the dropzone with `Tab` and press `Enter` or `Space` to open your system file dialog.
* **File picker button (`Choose WASM File`):** Click **Choose WASM File** to pick a `.wasm` binary from disk.
* **Bundled example buttons (`No contract handy?` panel):**
  * Click **`Load example: us_old.wasm`** (*transfer & mint — event + struct type*) to inspect the bundled 198-byte token-style fixture.
  * Click **`Load example: us_new.wasm`** (*hello() and increment() — candidate upgrade*) when no base contract is currently loaded (for example, on a fresh page or immediately after clicking **`reset`**) to inspect the 530-byte counter-style fixture on its own. *(Note: if a base contract is already inspected, clicking **`Load example: us_new.wasm`** loads `us_new.wasm` into the candidate comparison slot instead—see [Diff / Upgrade-Safety Flow](#diff--upgrade-safety-flow) below.)*

Once a contract is selected, the file bar appears beneath the upload row showing:

* The file name (bundled fixtures are labeled `example: us_old.wasm (bundled)` or `example: us_new.wasm (bundled)` so they are never confused with files on disk).
* File size and MIME type (for example, `198 B · application/wasm`).
* Live status (`inspecting…` followed by `inspection complete · <N> ms`).
* The **`reset`** button, which clears the inspected contract, any loaded candidate WASM, the status indicator, and all rendered report sections, returning keyboard focus to the dropzone.

### 2. Reading the Inspection Report Sections

Every successful inspection renders the following report sections in order:

| Report section | Question it answers | What `us_old.wasm` vs `us_new.wasm` shows |
| :--- | :--- | :--- |
| **Metadata** | *Which exact binary is this, what is its verifiable digest and size, and what top-level WASM header counts does it report?* | Displays `File`, `SHA-256` (with a one-click **`copy`** button next to the digest), `Size`, WASM `Version`, `Interface version` (when present on `spec.env_meta`), `Inspection time`, plus summary `Functions`, `Tables`, `Globals`, `Data segments`, and `Memory` rows. `us_old.wasm` hashes to `05befa136e7f0829a5051d97b032f355a5e65976397df90b224d141942dce46c` (`198 B`); `us_new.wasm` hashes to `551c5a9c7fd4c4a71e57e9c8a0ece1e7bd506ea065993c63db2dc35516406775` (`530 B`, `Memory: 16 initial pages`). *(Immediately beneath Metadata, **Size breakdown** plots horizontal byte/percentage bars per WASM section—`1 section` on `us_old.wasm` vs `10 sections` on `us_new.wasm`.)* |
| **Contract Metadata** | *What build/provenance key-value entries were recorded in `contractmetav0`?* | Renders decoded `SCMetaV0` entries verbatim in section order without filtering unknown or duplicate keys. `us_old.wasm` has `0 items` (`Entries: none`); `us_new.wasm` has `3 items` (`rsver: 1.97.1`, `rssdkver: 22.0.11#34f7f53ae31e0fd02aab436a9872e79fa671ca02`, `cliver: 27.1.0#8e402ea28202950b272fbabc34caad4d2f64fe87`). |
| **Exports** | *Which functions, memories, tables, or globals does the WASM module export across its boundary?* | `us_old.wasm` is a spec-only fixture with `0 items` (`Exports: none`); `us_new.wasm` lists `4 items`: `memory: memory`, `func: hello`, `func: increment`, and `func: _`. |
| **Imports** | *Which host functions does the contract import from the Soroban VM environment?* | `us_old.wasm` has `0 items` (`Imports: none`); `us_new.wasm` lists `3 items` imported from module `l`: `0 [func]`, `1 [func]`, and `_ [func]`. |
| **Custom Sections** | *Which Soroban custom sections (`contractspecv0`, `contractenvmetav0`, `contractmetav0`) are embedded in the binary?* | `us_old.wasm` contains `1 item` (`name: contractspecv0`); `us_new.wasm` contains `4 items` (`contractspecv0`, `contractenvmetav0`, and two `contractmetav0` sections). |
| **Structural Metrics** | *What are the low-level WebAssembly module counts and linear memory bounds?* | Summarizes `Functions (declared)`, `Memory` (initial and maximum pages), `Tables`, `Globals`, and `Data Segments`. `us_old.wasm` reports `0` functions and `Memory: none`; `us_new.wasm` reports `3` declared functions and `16 pages (max: unbounded)`. |
| **Contract Specification** | *What public Soroban ABI—callable functions, custom types, and events—does `contractspecv0` expose to callers and indexers?* | Grouped into **Functions**, **Custom Types**, and **Events**. `us_old.wasm` declares `transfer(to: address)` and `mint(amt: u32)`, custom struct `Point`, and event `Transfer`. `us_new.wasm` declares `hello() -> u32` and `increment() -> u32`, with `0` custom types and `0` events. |

At the bottom of a single-file inspection, the **Upgrade Safety** section starts in the `waiting` state (`Load a candidate WASM to compare it with the inspected base.`).

***

## Diff / Upgrade-Safety Flow

The **Compare an upgrade** card (`#compare`, also reachable via the **`Diff`** link in the top navigation bar) compares a candidate upgrade `.wasm` against the currently inspected base contract.

### Step-by-Step Comparison with the Bundled Examples

1. **Inspect the base contract:** Click **`Load example: us_old.wasm`** (or drop/choose your deployed `.wasm` in the dropzone). Wait for the file bar to report `inspection complete`.
2. **Load the candidate contract:** Either:
   * Click **`Choose candidate`** inside the **Compare an upgrade** card and select a candidate `.wasm` from disk (such as `website/playground/examples/us_new.wasm`), **or**
   * Click **`Load example: us_new.wasm`** while `us_old.wasm` is already loaded—the candidate slot updates from `No candidate loaded` to `example: us_new.wasm (bundled)` and runs the diff automatically.
3. **Read the `Upgrade Safety` verdict:** For `us_old.wasm` (base) → `us_new.wasm` (candidate), the file bar status updates to **`breaking changes detected`**, the **Upgrade Safety** section badge changes from `waiting` to **`breaking`**, and the report displays:
   * **`Compatible`**: `no`
   * **`Breaking changes`**:
     * `removed_function`: `Removed function: mint()`
     * `removed_function`: `Removed function: transfer()`
     * `removed_event`: `Removed event: Transfer`
     * `removed_type`: `Removed type: Point`
   * **`Non-breaking changes`**:
     * `added_function`: `Added function: hello()`
     * `added_function`: `Added function: increment()`
4. **Reset when finished:** Click **`reset`** in the file bar to clear both slots before starting an independent inspection.

### Interpreting the Verdict (and What It Does Not Claim)

* **Why `Compatible: no` is breaking:** Removing public functions (`mint`, `transfer`), changing their parameter/return signatures, removing an emitted event (`Transfer`), or removing/altering a public `contractspecv0` type (`Point`) breaks callers, downstream cross-contract invocations, and off-chain indexers that target the existing contract ID. Adding new functions (`hello()`, `increment()`) is classified under **Non-breaking changes** because existing call sites are unaffected.
* **What this verdict does NOT check:** The browser diff runs the exact same offline `contractspecv0` ABI comparison as `sdkt diff --upgrade-safety` in the CLI. It is **not** a full deployment gate: it does not verify storage key/layout migrations, authorization invariants, host resource budgets, WASM size limits, or live ledger state.

***

## Navigation Items Marked "Coming Soon"

Three items in the top navigation bar are rendered as disabled placeholders (`Coming soon — not implemented yet`) and are not interactive yet:

* **`Analyze`** — Intended for offline static security and artifact checks in the browser (available today via `sdkt audit`).
* **`XDR`** — Intended for browser-based Stellar/Soroban XDR decoding and encoding (available today via `sdkt decode` and `sdkt encode`).
* **`Health`** — Intended for contract readiness and storage/TTL health reporting (available today via `sdkt health` and `sdkt storage`).

***

## What the Web Playground Cannot Do

Because the Web Playground runs strictly in local browser memory with zero network/RPC access:

* **No on-chain contract loading by Contract ID** (`C...` addresses cannot be fetched from Soroban RPC in the playground).
* **No contract invocation or transaction simulation.**
* **No live contract health, storage rent, or TTL inspection.**
* **No identity management, envelope signing, or transaction submission.**

For all RPC-backed workflows, static security audits, fuzzing campaigns, and CI automation, use the `sdkt` CLI—see the [CLI Command Reference](../reference/cli.md).

***

## For Contributors: Rebuilding the Browser WASM Runtime

The prebuilt WebAssembly bundle (`website/playground/wasm/sdkt_playground.js` and `sdkt_playground_bg.wasm`) is committed to the repository so static hosting requires no build step. If you modify `crates/sdkt-playground` or `crates/sdkt-wasm`, regenerate the browser artifacts from the repository root with:

```bash
rustup toolchain install 1.88.0 --target wasm32-unknown-unknown
cargo +1.88.0 install wasm-bindgen-cli --version 0.2.127 --locked
bash website/playground/build.sh
```

CI (`playground WASM freshness` in `.github/workflows/ci.yml`) rebuilds `crates/sdkt-playground` with Rust `1.88.0` and `wasm-bindgen` `0.2.127` and verifies byte-for-byte equality with `website/playground/wasm/`.
