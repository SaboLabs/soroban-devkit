# CI/CD with Soroban DevKit (`sdkt`)

`sdkt` ships a reusable **GitHub composite Action** at
[`sdkt` composite Action](../../.github/actions/sdkt/action.yml) so you can
gate merges on static security findings (`sdkt audit`), on breaking contract
upgrades (`sdkt diff --upgrade-safety`), on a read-only release verdict
(`sdkt release-assurance`), and on post-deploy on-chain contract posture
(`sdkt health`) — entirely in CI, no local install required.

The Action installs a pinned `sdkt` binary, runs the chosen subcommand in JSON
mode, and fails the step when the check does not pass.

## Inputs

| Input | Required | Default | Meaning |
|-------|----------|---------|---------|
| `command` | yes | — | `audit`, `upgrade-safety`, `release-assurance`, or `health` |
| `sdkt-version` | no | `v2.7.0` | Pinned `sdkt` git tag to install |
| `install-from-source` | no | `"false"` | Build sdkt from the pinned git tag instead of downloading the prebuilt release asset. Slower (~4-5 min) but resolves: unsupported kernel, missing prebuilt asset for the runner OS/arch, missing release asset for the version, missing checksum, or missing `sha256sum`/`shasum` tool. |
| `target` | for `audit` | `""` | Path to the `.rs` source to audit |
| `old-wasm` | for `upgrade-safety` | `""` | Baseline (currently deployed) WASM |
| `new-wasm` | for `upgrade-safety` | `""` | Candidate (new) WASM |
| `severity-threshold` | no | `critical` | `critical` \| `warning` \| `info` |
| `wasm` | for `release-assurance` (optional for `health`) | `""` | Candidate WASM artifact to assess (`release-assurance`), or optional local WASM path to verify against the on-chain code hash (`health`) |
| `previous-wasm` | no | `""` | Baseline WASM for the offline upgrade diff |
| `audit` | no | `""` | Rust source path(s)/dir(s) for the static audit (space-separated) |
| `disable` | no | `""` | Audit rule id(s) to disable (space-separated) |
| `contract` | for `health` (optional for `release-assurance`) | `""` | Deployed contract ID (C...) for on-chain checks; required for `health`, skipped when empty for `release-assurance` |
| `network` | for `health` (optional for `release-assurance`) | `""` | `testnet` \| `mainnet` \| `futurenet` for the on-chain checks |
| `health-threshold` | no | `critical` | `critical` \| `at_risk` (used with `command=health`) |
| `max-size-bytes` | for `release-assurance` | `""` | Fail the release when the candidate artifact exceeds N bytes (forwards `--max-size-bytes`) |
| `max-growth-pct` | for `release-assurance` | `""` | Fail the release when the candidate grew more than N percent over `previous-wasm` (forwards `--max-growth-pct`) |

## Outputs

The Action writes step outputs via `$GITHUB_OUTPUT` and a markdown table to `$GITHUB_STEP_SUMMARY` for every command.

### `release-assurance` outputs

| Output | Type | Meaning |
|--------|------|---------|
| `release_status` | `PASS` \| `REVIEW` \| `FAIL` | Aggregate verdict from the report's `.release_status` field. Written even when the step fails so an `if: failure()` consumer can read it. |

### `audit` outputs

| Output | Type | Meaning |
|--------|------|---------|
| `critical` | integer (string) | Count of critical-severity findings from `.summary.critical`. |
| `warning` | integer (string) | Count of warning-severity findings from `.summary.warning`. |
| `info` | integer (string) | Count of info-severity findings from `.summary.info`. |

All three are written before the gate runs, so they are available to `if: failure()` steps.

### `upgrade-safety` outputs

| Output | Type | Meaning |
|--------|------|---------|
| `compatible` | `true` \| `false` | Upgrade-safety verdict from `.compatible`. Written before the gate so a failing step's output is still readable. |

### Reading outputs in a later step

```yaml
- name: Check upgrade compatibility
  id: compat
  uses: SaboLabs/soroban-devkit/.github/actions/sdkt@main
  with:
    command: upgrade-safety
    old-wasm: builds/current.wasm
    new-wasm: builds/candidate.wasm

- name: Post verdict as a comment
  if: always()
  run: echo "compatible=${{ steps.compat.outputs.compatible }}"
```

```yaml
- name: Release assurance
  id: ra
  uses: SaboLabs/soroban-devkit/.github/actions/sdkt@main
  with:
    command: release-assurance
    wasm: builds/candidate.wasm

- name: Gate downstream job
  if: always()
  run: echo "release_status=${{ steps.ra.outputs.release_status }}"
```

**Threshold semantics:** only findings at or above `severity-threshold` fail
the build. The default `critical` means `MOVE-001` (Warning) never breaks CI.
The Action forwards the input as the CLI's own flag —
`sdkt audit <target> --format json --fail-on "$severity-threshold"` — and gates
on the exit code, so the CLI is the single source of truth for the mapping:
exit `1` means a finding at or above the threshold, and the full JSON report is
still printed first. An unknown level is rejected by the CLI as a usage error
(exit `2`), which fails the step with a clear message instead of silently
running as a no-op. No threshold logic is reimplemented in shell, and the audit
gate does not need `jq`.

**Release-assurance semantics:** the aggregated `release_status` gates the
step. `FAIL` blocks (step exits 1); `PASS`, `REVIEW`, and `SKIPPED`-only
results do **not** block (exit 0), matching the CLI. A breaking baseline
therefore fails CI, while a report that only needs human review passes. A
CLI error that produces no report at all (invalid input, unreadable artifact,
RPC failure) also fails the step: the Action propagates the CLI's exit code
rather than relying on `release_status` alone.

**Size policy:** `max-size-bytes` and `max-growth-pct` are optional and
forwarded to the CLI's existing size policy. Thresholds are
operator-supplied — SDKT hardcodes no network limit. Boundaries are
inclusive, so a size exactly at the limit or growth exactly at the cap
passes. `max-growth-pct` requires `previous-wasm`; without it the CLI
fails rather than silently skipping the check. Omitting both inputs
leaves the release-assurance result unchanged.

**Health semantics:** `command=health` runs
`sdkt health --contract <contract> --network <network> [--wasm <file>] --format json`,
prints the JSON report, and gates on the parsed fields (`.health` and, when
`wasm` is supplied, `.verified`). At the default `health-threshold: critical`,
only `.health == "critical"` fails the step (`at_risk` passes); when
`health-threshold` is set to `at_risk`, both `at_risk` and `critical` fail.
When `wasm` is provided, `.verified == false` (on-chain vs. local WASM hash
mismatch) also fails the step with an explicit mismatch error; when `wasm` is
omitted, no hash comparison runs. An unknown `health-threshold` value is
rejected before `sdkt` is invoked, and a nonexistent contract ID fails with the
CLI's not-found error rather than a `jq` parse failure.

**Read-only:** `release-assurance` and `health` never sign, submit, deploy,
extend TTL, or mutate state, and the Action passes no credentials.

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
          sdkt-version: v2.7.0
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
          sdkt-version: v2.7.0
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
          sdkt-version: v2.7.0
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

## Example 4 — Post-deploy contract health smoke check

Verify that a deployed contract exists on-chain, matches the local `.wasm`
artifact you built, and has healthy storage TTL posture:

```yaml
# .github/workflows/sdkt-health.yml
name: sdkt Post-Deploy Health

on:
  workflow_dispatch:

jobs:
  health:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Post-deploy contract health check
        uses: SaboLabs/soroban-devkit/.github/actions/sdkt@main
        with:
          command: health
          sdkt-version: v2.7.0
          contract: CCJUD55AG6W5HAI5LRVNKAE5WDP5XGZBUDS5WNTIVDU7O264UZZE7BRD
          network: testnet
          wasm: builds/candidate.wasm
          health-threshold: critical
```

`contract` and `network` are required. `wasm` is optional (omit it to check
liveness and storage TTL without a local code-hash comparison), and
`health-threshold` defaults to `critical` (set `at_risk` to also fail when
storage entries are expiring soon or empty).

## Example 5 — Self-validating the Action (this repo)

This repository validates the composite Action itself in
[`.github/workflows/sdkt-action-ci.yml`](../../.github/workflows/sdkt-action-ci.yml):
a **breaking** diff (committed fixtures `us_old.wasm` → `us_new.wasm`) is
asserted to **fail**, an **identical** diff is asserted to **pass**, a
release-assurance run against that same breaking baseline is asserted to
**fail** (`release_status=FAIL`), and a candidate-only run is asserted to
**pass** (`release_status=REVIEW`). The audit gate is asserted both ways too: a
fixture with a critical finding must fail, a clean fixture must pass, and an
unknown `severity-threshold` must be rejected. A dedicated `validate-health`
job exercises `command: health` against live Soroban Testnet contracts (healthy
pass, `health-threshold` gating for `healthy` and `at_risk` contracts, invalid
threshold rejection, `--wasm` hash-mismatch failure, and nonexistent contract
failure).

The `assert-outputs` job validates the `outputs:` section end-to-end: it runs
each command mode against the same committed fixtures, then asserts in a
**separate later step** (using `${{ steps.<id>.outputs.<name> }}`) that:

- `steps.ra_out.outputs.release_status` equals `REVIEW` after a no-baseline
  release-assurance run,
- `steps.audit_out.outputs.critical`, `.warning`, and `.info` all equal `0`
  after a clean audit run,
- `steps.us_out.outputs.compatible` equals `true` after an identical-WASM
  upgrade-safety run, and
- `steps.ra_fail_out.outputs.release_status` equals `FAIL` even after a
  gate-failing release-assurance run (`continue-on-error: true` lets the
  assertion step run despite the gate having failed).

## Notes

- The Action installs `sdkt` from a pinned git tag via
  `cargo install --git https://github.com/SaboLabs/soroban-devkit --tag <sdkt-version> sdkt-cli --locked`
  (or, when run inside the sdkt workspace itself, from the local path). For
  faster, reproducible CI, pin to a released tag and consider a prebuilt-binary
  install mode (future optimization).
- `upgrade-safety` requires the **baseline** WASM to be supplied explicitly
  (`old-wasm`); it does not fetch the on-chain deployed contract. Provide your
  previously-deployed `.wasm` as the baseline artifact.
- JSON output of `sdkt diff --upgrade-safety`, `sdkt release-assurance`, and
  `sdkt health` is the stable contract the Action parses (all are additive and
  serde-derived); those three gates are also why the Action still provisions
  `jq`; the provisioning step is skipped entirely when `command` is `audit`.
  `sdkt audit` is gated by the CLI's exit code through `--fail-on`, so the
  audit path does not run `jq` at all.
- **Outputs are always written before the gate exits**, so a step with
  `if: failure()` or `if: always()` can still read them (e.g. to post the
  verdict as a PR comment or gate a downstream notification job).
- **Step summaries** appear in the Checks tab of every run: a one-line verdict
  plus a table of section statuses / finding counts built from the same parsed
  JSON — no extra CLI flags or tools required.
