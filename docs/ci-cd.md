# CI/CD with Soroban DevKit (`sdkt`)

`sdkt` ships a reusable **GitHub composite Action** at
[`.github/actions/sdkt/action.yml`](.github/actions/sdkt/action.yml) so you can
gate merges on static security findings (`sdkt audit`) and on breaking contract
upgrades (`sdkt diff --upgrade-safety`) — entirely in CI, no local install
required.

The Action installs a pinned `sdkt` binary, runs the chosen subcommand in JSON
mode, and fails the step when the check does not pass.

## Inputs

| Input | Required | Default | Meaning |
|-------|----------|---------|---------|
| `command` | yes | — | `audit` or `upgrade-safety` |
| `sdkt-version` | no | `v2.1.1` | Pinned `sdkt` git tag to install |
| `target` | for `audit` | `""` | Path to the `.rs` source to audit |
| `old-wasm` | for `upgrade-safety` | `""` | Baseline (currently deployed) WASM |
| `new-wasm` | for `upgrade-safety` | `""` | Candidate (new) WASM |
| `severity-threshold` | no | `critical` | `critical` \| `warning` \| `info` |

**Threshold semantics:** only findings at or above `severity-threshold` fail
the build. The default `critical` means `MOVE-001` (Warning) never breaks CI.

## Example 1 — Audit on every PR

```yaml
# .github/workflows/sdkt-audit.yml
name: sdkt Audit

on:
  pull_request:
  push:
    branches: [main]

jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Static security audit
        uses: naninu123/soroban-devkit/.github/actions/sdkt@main
        with:
          command: audit
          sdkt-version: v2.1.1
          target: contracts/token/src/lib.rs
          severity-threshold: critical
```

## Example 2 — Upgrade safety on release

Provide the currently-deployed WASM (`old-wasm`) and the candidate
(`new-wasm`). The step fails when the upgrade is not backwards-compatible.

```yaml
# .github/workflows/sdkt-upgrade-safety.yml
name: sdkt Upgrade Safety

on:
  release:
    types: [published]

jobs:
  upgrade-safety:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Check upgrade compatibility
        uses: naninu123/soroban-devkit/.github/actions/sdkt@main
        with:
          command: upgrade-safety
          sdkt-version: v2.1.1
          old-wasm: builds/current.wasm
          new-wasm: builds/candidate.wasm
```

## Example 3 — Self-validating the Action (this repo)

This repository validates the composite Action itself in
[`.github/workflows/sdkt-action-ci.yml`](.github/workflows/sdkt-action-ci.yml):
a **breaking** diff (committed fixtures `us_old.wasm` → `us_new.wasm`) is
asserted to **fail**, and an **identical** diff is asserted to **pass**.

## Notes

- The Action installs `sdkt` from a pinned git tag via
  `cargo install --git https://github.com/naninu123/soroban-devkit --tag <sdkt-version> sdkt-cli --locked`
  (or, when run inside the sdkt workspace itself, from the local path). For
  faster, reproducible CI, pin to a released tag and consider a prebuilt-binary
  install mode (future optimization).
- `upgrade-safety` requires the **baseline** WASM to be supplied explicitly
  (`old-wasm`); it does not fetch the on-chain deployed contract. Provide your
  previously-deployed `.wasm` as the baseline artifact.
- JSON output of `sdkt audit` and `sdkt diff --upgrade-safety` is the stable
  contract the Action parses; both are additive and serde-derived.
