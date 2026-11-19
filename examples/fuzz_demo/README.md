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
pattern into real contracts (multiply first, or use checked arithmetic).
This is the contract's one and only deliberate defect.

## Build the WASM

Standard Soroban toolchain (Rust + the `wasm32v1-none` target):

```bash
cargo build --release --target wasm32v1-none
```

This produces `target/wasm32v1-none/release/fuzz_demo.wasm`. The `.wasm`
is **not committed** — `run.sh` builds it.

Prerequisites:

```bash
rustup target add wasm32v1-none
cargo build --bin sdkt --release        # from the repo root
```

## Run the fuzz campaign

```bash
sdkt fuzz campaign target/wasm32v1-none/release/fuzz_demo.wasm \
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
artifact finding-case0000-unexpected_error.json canonical_hash=f8e4ce7980b7b12afee4ef6412ff53c5d5ca813cd9d8dec0b6c3135cf9d0b601
evidence: completed=true findings=1 minimized=1 replayed=0 deterministic=true
==> OK: expected UNEXPECTED_ERROR finding surfaced
```

The campaign exits `1` when findings exist (the only exit code that means
"a declared rule was violated"), so a clean checkout that reproduces this
output proves the workflow works.

## run.sh

`run.sh` rebuilds the WASM, runs the campaign above, and asserts the
`UNEXPECTED_ERROR` finding appears in the output (exit 0) or exits
non-zero:

```bash
bash examples/fuzz_demo/run.sh
```

## Relationship to #340

Issue #340 (make `examples/sample_token` buildable) has not landed, so
this example defines the buildable-example pattern here rather than
following it:

- standard `soroban-sdk` contract (`#[contract]` / `#[contractimpl]`)
- `cdylib` crate type
- `[lib] crate-type = ["cdylib"]`, release profile optimized for size
- standalone crate (its own `[workspace]` table) so `cargo build
  --workspace` in the parent repo stays a pure native build

The `Cargo.toml` pins `soroban-sdk = "=27.0.6"`, matching the version the
repo's test fixtures already use. When #340 lands, `sample_token`'s
`Cargo.toml` should mirror this pattern; no changes to this demo are
needed then.
