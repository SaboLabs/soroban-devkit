# CI/CD with Soroban DevKit (`sdkt`)

`sdkt` ships a reusable **GitHub composite Action** at
[`sdkt` composite Action](../../.github/actions/sdkt/action.yml) so you can
gate merges on static security findings (`sdkt audit`), on breaking contract
upgrades (`sdkt diff --upgrade-safety`), and on a read-only release verdict
(`sdkt release-assurance`) — entirely in CI, no local install required.

The Action installs a pinned `sdkt` binary, runs the chosen subcommand in JSON
mode, and fails the step when the check does not pass.

## Inputs

| Input | Required | Default | Meaning |
|-------|----------|---------|---------|
| `command` | yes | — | `audit`, `upgrade-safety`, or `release-assurance` |
| `sdkt-version` | no | `v2.6.0` | Pinned `sdkt` git tag to install |
| `target` | for `audit` | `""` | Path to the `.rs` source to audit |
| `old-wasm` | for `upgrade-safety` | `""` | Baseline (currently deployed) WASM |
| `new-wasm` | for `upgrade-safety` | `""` | Candidate (new) WASM |
| `severity-threshold` | no | `critical` | `critical` \| `warning` \| `info` |
| `wasm` | for `release-assurance` | `""` | Candidate WASM artifact to assess |
| `previous-wasm` | no | `""` | Baseline WASM for the offline upgrade diff |
| `audit` | no | `""` | Rust source path(s)/dir(s) for the static audit (space-separated) |
| `disable` | no | `""` | Audit rule id(s) to disable (space-separated) |
| `contract` | no | `""` | Deployed contract ID (C...) for on-chain checks; skipped when empty |
| `network` | no | `""` | `testnet` \| `mainnet` \| `futurenet` for the on-chain checks |
| `max-size-bytes` | for `release-assurance` | `""` | Fail the release when the candidate artifact exceeds N bytes (forwards `--max-size-bytes`) |
| `max-growth-pct` | for `release-assurance` | `""` | Fail the release when the candidate grew more than N percent over `previous-wasm` (forwards `--max-growth-pct`) |

**Threshold semantics:** only findings at or above `severity-threshold` fail
the build. The default `critical` means `MOVE-001` (Warning) never breaks CI.

**Release-assurance semantics:** the aggregated `release_status` gates the
step. `FAIL` blocks (step exits 1); `PASS`, `REVIEW`, and `SKIPPED`-only
results do **not** block (exit 0), matching the CLI. A breaking baseline
therefore fails CI, while a report that only needs human review passes.

**Size policy:** `max-size-bytes` and `max-growth-pct` are optional and
forwarded to the CLI's existing size policy. Thresholds are
operator-supplied — SDKT hardcodes no network limit. Boundaries are
inclusive, so a size exactly at the limit or growth exactly at the cap
passes. `max-growth-pct` requires `previous-wasm`; without it the CLI
fails rather than silently skipping the check. Omitting both inputs
leaves the release-assurance result unchanged.

**Read-only:** `release-assurance` never signs, submits, deploys, extends TTL,
or mutates state, and the Action passes no credentials.

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
        uses: SaboLabs/soroban-devkit/.github/actions/sdkt@main
        with:
          command: audit
          sdkt-version: v2.6.0
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
        uses: SaboLabs/soroban-devkit/.github/actions/sdkt@main
        with:
          command: upgrade-safety
          sdkt-version: v2.6.0
          old-wasm: builds/current.wasm
          new-wasm: builds/candidate.wasm
```

## Example 3 — Release assurance before publishing

Assess the candidate artifact as a whole: artifact metadata, static audit,
upgrade safety, and (optionally) on-chain verification and health. The step
fails only when `release_status` is `FAIL`.

```yaml
# .github/workflows/sdkt-release-assurance.yml
name: sdkt Release Assurance

on:
  pull_request:
    branches: [main]

jobs:
  release-assurance:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Release assurance
        uses: SaboLabs/soroban-devkit/.github/actions/sdkt@main
        with:
          command: release-assurance
          sdkt-version: v2.6.0
          wasm: builds/candidate.wasm
          previous-wasm: builds/current.wasm
          audit: contracts/token/src/lib.rs
          disable: MOVE-001
```

`wasm` is required. `previous-wasm`, `audit`, `disable`, `contract`, and
`network` are optional: `previous-wasm` supplies the offline upgrade baseline,
`audit`/`disable` drive the static security section, and `contract`/`network`
add the on-chain verification and health sections (skipped entirely when
`contract` is empty). Adding `contract` and `network` performs read-only RPC
reads only.

## Example 4 — Self-validating the Action (this repo)

This repository validates the composite Action itself in
[`.github/workflows/sdkt-action-ci.yml`](../../.github/workflows/sdkt-action-ci.yml):
a **breaking** diff (committed fixtures `us_old.wasm` → `us_new.wasm`) is
asserted to **fail**, an **identical** diff is asserted to **pass**, a
release-assurance run against that same breaking baseline is asserted to
**fail** (`release_status=FAIL`), and a candidate-only run is asserted to
**pass** (`release_status=REVIEW`).

## Notes

- The Action installs `sdkt` from a pinned git tag via
  `cargo install --git https://github.com/SaboLabs/soroban-devkit --tag <sdkt-version> sdkt-cli --locked`
  (or, when run inside the sdkt workspace itself, from the local path). For
  faster, reproducible CI, pin to a released tag and consider a prebuilt-binary
  install mode (future optimization).
- `upgrade-safety` requires the **baseline** WASM to be supplied explicitly
  (`old-wasm`); it does not fetch the on-chain deployed contract. Provide your
  previously-deployed `.wasm` as the baseline artifact.
- JSON output of `sdkt audit`, `sdkt diff --upgrade-safety`, and
  `sdkt release-assurance` is the stable contract the Action parses; all are
  additive and serde-derived.
