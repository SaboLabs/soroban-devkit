# fuzz_demo — `sdkt fuzz` workflow demo

A minimal, self-contained Soroban contract that turns the `sdkt fuzz`
workflow's first run into a verifiable success: one campaign invocation
that deterministically surfaces a finding.

## The contract

`src/lib.rs` is a single-function Soroban contract:

```rust
pub fn compute(a: i128, b: i128) -> i128 { (a / b) * a }
```

## The deliberate defect

`compute` divides `a` by `b` *before* multiplying by `a` — the
division-before-multiplication precision-loss pattern flagged by the
`MATH-001` rule ("Division before multiplication may truncate numeric
precision").

The defect is **intentional** and documented as such in `src/lib.rs`
(`// INTENTIONAL DEFECT: divide before multiply (MATH-001 pattern).`). It
exists only to give `sdkt fuzz` something to find; do **not** copy this
pattern into real contracts. To preserve precision, multiply before
dividing and handle possible overflow — checked arithmetic alone does not
recover a fraction that division has already discarded (`checked_div`
followed by `checked_mul` still truncates). This is the contract's one and
only deliberate defect.

## Build the `sdkt` CLI (from the repository root)

`cargo build --bin sdkt --release` produces `target/release/sdkt` (or
`sdkt.exe` on Windows) at the repository root — it does **not** install the
binary onto `PATH`. Export an absolute path so the later commands work from
any directory:

```bash
cargo build --bin sdkt --release
export SDKT="$(pwd)/target/release/sdkt"   # use sdkt.exe on Windows
```

## Build the WASM (from `examples/fuzz_demo`)

From the repository root, `cd` into the example before building — Cargo
selects its manifest from the current directory, and this contract is a
standalone crate (its own `[workspace]` table), not a workspace member:

```bash
cd examples/fuzz_demo
rustup target add wasm32v1-none
cargo build --release --target wasm32v1-none
```

This produces `examples/fuzz_demo/target/wasm32v1-none/release/fuzz_demo.wasm`
(relative paths below are from `examples/fuzz_demo`). The `.wasm` is **not
committed** — `run.sh` builds it.

## Run the fuzz campaign (from `examples/fuzz_demo`)

Stay in `examples/fuzz_demo` (the directory from the previous section) and
use the absolute `$SDKT` path exported above:

```bash
"$SDKT" fuzz campaign target/wasm32v1-none/release/fuzz_demo.wasm \
  --seed 0 --cases 1 --function compute --auth no-auth \
  --expect-success compute --format pretty
```

`--expect-success compute` declares every call must succeed. The fuzzer's
`zero` argument mutation drives `b` to zero, so `a / b` (the deliberate
defect) traps the VM; since success was declared, the trap is reported as
an `UNEXPECTED_ERROR` finding — the campaign's "did it work?" signal.

Expected output (verbatim from `run.sh`):

```
==> Building target/wasm32v1-none/release/fuzz_demo.wasm
==> Running fuzz campaign (seed=0, cases=1, function=compute)
Campaign summary: executed=1 passed=0 expected_errors=0 findings=1 skipped=0
FINDING case0000 compute UNEXPECTED_ERROR (minimized_complexity=16)
artifact finding-case0000-unexpected_error.json canonical_hash=f517bed757075918ba7780687ff1aa15e2e581946bdf1eafb4c8e26947b319c7
evidence: completed=true findings=1 minimized=1 replayed=0 deterministic=true
==> OK: expected UNEXPECTED_ERROR finding surfaced
```

`canonical_hash` is derived from the compiled WASM bytes, so its exact value
can differ with the Rust toolchain version; the `FINDING ... UNEXPECTED_ERROR`
line is the stable, asserted signal (repeated runs on the same toolchain are
byte-identical, as `deterministic=true` reports).

The campaign exits `1` when findings exist (the only exit code that means
"a declared rule was violated"), so a clean checkout that reproduces this
output proves the workflow works.

## run.sh (from the repository root)

`run.sh` rebuilds the WASM, runs the campaign above, and asserts the
`UNEXPECTED_ERROR` finding appears in the output (exit 0) or exits
non-zero. It resolves its own directory, so the working directory does not
matter; pass the absolute binary path via `SDKT` since `sdkt` is not on
`PATH`:

```bash
cd /path/to/soroban-devkit   # repository root
SDKT="$(pwd)/target/release/sdkt" bash examples/fuzz_demo/run.sh
```

CI executes this script on every pull request via the `fuzz-demo` job in
`.github/workflows/ci.yml`, so the transcript above is machine-verified,
not just an author claim.

## Relationship to #340

Issue #340 (make `examples/sample_token` buildable) has not landed, so
this example defines the buildable-example pattern here rather than
following it:

- standard `soroban-sdk` contract (`#[contract]` / `#[contractimpl]`)
- `cdylib` crate type
- `[lib] crate-type = ["cdylib"]`, release profile optimized for size
- standalone crate (its own `[workspace]` table) so `cargo build
  --workspace` in the parent repo stays a pure native build
- its own committed `Cargo.lock` pins the standalone crate's full
  dependency tree (the root workspace lockfile does not cover it), so the
  example builds reproducibly

The `Cargo.toml` pins `soroban-sdk = "=27.0.6"`, matching the version the
repo's test fixtures already use. When #340 lands, `sample_token`'s
`Cargo.toml` should mirror this pattern; no changes to this demo are
needed then.
