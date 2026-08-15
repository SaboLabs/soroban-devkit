# sdkt onboarding examples

These files back the offline smoke test (`scripts/smoke_examples.sh`) and the
documented quick start in `docs/examples.md`. They are intentionally tiny and
self-contained.

## `sample_token/src/lib.rs`

A minimal Soroban contract used to demonstrate `sdkt audit`.

- `transfer` is a correctly guarded privileged entrypoint (calls `require_auth()`).
- `admin_action` is **deliberately** left unguarded so that `sdkt audit` produces
  a deterministic **AUTH-001** finding. This is the point of the example — it shows
  the static analyzer catching a missing auth check. Do **not** copy this pattern
  into real contracts.

The file is analyzed as Rust source only; it does not need to be compiled for the
smoke test (which runs `sdkt audit` against the source file directly).

## `sample_scval.b64`

A single valid base64-encoded `ScVal` (`{"bool": false}`) used to demonstrate
`sdkt decode --type ScVal` without needing a network.

## Running the smoke test

```bash
cargo build --bin sdkt
bash scripts/smoke_examples.sh
```

The script asserts: version 2.5.0, a contract spec in `us_old.wasm`, an
AUTH-001 finding on `admin_action`, and a `{"bool": false}` decode.
