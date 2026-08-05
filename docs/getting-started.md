# Getting Started

This guide takes you from zero to your first `sdkt` commands. Everything here
runs **offline** — no RPC node, no network — so you can follow along in under
five minutes.

## 1. Install

See [installation.md](installation.md). The fastest path:

```bash
# From source
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli

# Verify
sdkt --version
```

> The `sdkt` binary is the `sdkt-cli` crate. After `cargo install` the command
> is named `sdkt` (a symlink is created on PATH).

## 2. Your first command — offline ABI/WASM diff

`sdkt diff` compares two contract WASM files and reports added/removed
functions, changed signatures, events, and custom types. It needs no network.

```bash
sdkt diff \
  --old-wasm crates/sdkt-cli/tests/fixtures/us_old.wasm \
  --new-wasm crates/sdkt-cli/tests/fixtures/us_new.wasm
```

Sample output:

```
Contract WASM Diff
  OLD: 05befa136e7f0829a5051d97b032f355a5e65976397df90b224d141942dce46c (198 bytes)
  NEW: 5ae0c8b47b5723898bf9313abe1643f89eb23f19b9bd0cd82769db522767d97e (238 bytes)

Added functions (1):
  + balance (balance(who: address) -> void)
Changed signatures (1):
  ~ mint :
      old: mint(amt: u32) -> void
      new: mint(amt: u64) -> void
...
```

Add `--upgrade-safety` to get a breaking-change verdict (used by CI on
releases):

```bash
sdkt diff \
  --old-wasm a.wasm --new-wasm b.wasm \
  --upgrade-safety --format json
```

## 3. Your second command — static security audit

`sdkt audit` scans a Soroban contract's Rust source for common auth bugs:

```bash
sdkt audit path/to/contract/src/lib.rs
```

It flags `AUTH-001/002/003` (missing `require_auth` on privileged functions,
unauthenticated `invoke_contract`, unguarded `initialize`) and `MOVE-001`
(suspicious move-after-use, warning only). To skip a rule:

```bash
sdkt audit contract/src/lib.rs --disable MOVE-001
```

## 4. Where to go next

- [examples.md](examples.md) — copy-paste recipes for every subcommand.
- [installation.md](installation.md) — build options, features, updating.
- [cli.md](cli.md) — full command reference.
- [ci-cd.md](ci-cd.md) — gate your PRs on `sdkt audit` / upgrade-safety.
- [plugin-authoring.md](plugin-authoring.md) — extend `sdkt audit` with rules.
