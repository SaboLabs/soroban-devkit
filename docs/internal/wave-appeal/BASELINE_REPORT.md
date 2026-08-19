# BASELINE_REPORT.md

Captured: 2026-08-19
Repo: SaboLabs/soroban-devkit
Local path: `/home/ubuntu/soroban-devkit`
Classification: VERIFIED unless marked INFERENCE / ABSENT / UNKNOWN.

## Snapshot

| Field | Value | Source |
|---|---|---|
| Branch | `main` | `git branch --show-current` |
| Local HEAD | `6babff88d718ff780fb35ee388af89019e17d455` | `git rev-parse HEAD` |
| `origin/main` | `3f7d3956b362d4ce86f757edcab2ae5520c60416` | `git rev-parse origin/main` |
| Ahead/behind | local **ahead 1**, behind 0 | `git rev-list --left-right --count origin/main...HEAD` |
| Working tree | **clean** (no uncommitted files) | `git status --short` empty |
| Unpushed commit | `6babff8 chore: complete SCF award-readiness remediation (M50-M56)` (2026-08-15) | 9 files: CI onboarding-smoke, docs/examples, examples/, scripts/smoke_examples.sh, docs/scf.md |
| Workspace version | `2.5.0` | `Cargo.toml` `[workspace.package]` |
| Latest tag | `v2.5.0` @ `7bd1eb28` 2026-08-08 01:57 UTC | `git describe`; GitHub Releases |
| Commits on `origin/main` after tag | **34** (35 including local unpushed) | `git log v2.5.0..origin/main` |
| First commit | `84516d1` 2026-07-31 | `git log --reverse` |
| GitHub created | 2026-08-03T23:41:42Z | GitHub API |
| Last GitHub push | 2026-08-15T01:28:37Z (`origin/main`) | GitHub API |
| License | MIT | LICENSE + API |
| Language | Rust 2021, MSRV 1.88.0 | Cargo.toml |
| Homepage | https://sabolabs.github.io/soroban-devkit/ | GitHub API + live HTTP 200 |

**Phase 0 STOP check:** working tree is clean. The one local commit is already committed, not uncommitted dirt. It does **not** block Phase 1+. Do **not** reset/overwrite it. Do **not** push unless the operator authorizes.

## Technical maturity — VERIFIED

- Cargo virtual workspace: 8 members (`sdkt-core`, `sdkt-xdr`, `sdkt-cli`, `sdkt-rpc`, `sdkt-storage`, `sdkt-wasm`, `sdkt-audit`, `sdkt-audit-example-rule`) + excluded `sdkt-playground` (wasm32 glue).
- ~25,245 lines of Rust under `crates/` (excluding `target/`).
- 102 `*.rs` files; **482** `#[test]` / `#[tokio::test]` attributes (crate split: cli 188, core 113, xdr 46, audit 40, rpc 34, storage 26, wasm 25, playground 8, example-rule 2).
- `RELEASE_READINESS.md` claims 195 passed / 0 failed / 1 ignored at v2.5.0 — that number is a **doc snapshot**, not re-run in this Phase 0 pass.
- Live CLI (`./target/debug/sdkt --version`) prints `sdkt 2.5.0`. `--help` lists: decode, storage, inspect, verify, health, tx, events, account, fee, wasm, diff, audit, identity, network, init, deploy, build, lock, package, project, plugin, completions.
- CI workflows: `ci.yml` (fmt, clippy `-D warnings`, test matrix Ubuntu/macOS/Windows, MSRV, install.sh, supply-chain best-effort, **onboarding-smoke only on local HEAD**), `compatibility.yml` (clones `stellar/soroban-examples`), `pages.yml`, `release.yml`, `sdkt-action-ci.yml`.
- Latest CI on `origin/main`: success, run `31856606475` (2026-08-15). Compatibility workflow also success same SHA.
- GitHub Pages live 200: landing + `/playground/`. Last Pages deploy 2026-08-14 00:31 (ownership rename commit) — playground files exist on `origin/main`.
- Dockerfile + `install.sh` (Linux/macOS checksummed binary install). Composite Action at `.github/actions/sdkt/`.
- Examples: `examples/sample_token/src/lib.rs` (deliberate AUTH-001) + `examples/sample_scval.b64` — present **locally** in unpushed commit; **not on origin/main yet**.

## Stellar / Soroban relevance — VERIFIED FROM SOURCE (not README)

Grep of `crates/**/*.rs` hits real `parse_contract_spec`, `ContractSpec`, `UpgradeVerdict`, `get_wasm_bytecode`, AUTH-001 tests. CLI routes those to user commands. Compatibility CI clones official `stellar/soroban-examples`. Network default passphrase in README is the real Test SDF string.

This is a Soroban developer toolkit in code, not a rebranded generic CLI.

## Documentation quality

**Strong:** README (problem → capabilities → install → commands → workflows), `docs/quick-start.md`, `docs/examples.md`, `docs/installation.md`, `docs/cli.md`, `docs/ci-cd.md`, `docs/compatibility.md`, `docs/plugin-authoring.md`, CONTRIBUTING, SECURITY, SUPPORT, CODE_OF_CONDUCT, CHANGELOG (Keep a Changelog), ROADMAP, `docs/scf.md` (honest traction section).

**Weak / inaccurate (reviewer-visible):**

| Issue | Evidence | Class |
|---|---|---|
| README lists Windows asset `sdkt-x86_64-pc-windows-msvc.zip` | v2.5.0 GitHub Release assets = linux-x64 + darwin-x64 + darwin-arm64 **only** | P0 credibility |
| CONTRIBUTING.md: "Release binaries are provided for all three platforms" | Same; Windows target exists in **current** `release.yml` on main, not in published tag | P0 |
| README / quick-start omit Playground + github.io | Playground live; README only mentions `website/` folder | P0 discoverability |
| `docs/quick-start.md` install table has no Windows | Contradicts README | P1 |
| SUPPORT.md example `sdkt 2.4.0` | Current version 2.5.0 | P2 |
| `CHANGELOG.md` `[Unreleased]` only records `sdkt-wasm` dep removal | M40–M44, playground, website, Windows workflow, RPC live-compat fixes all landed after the tag and are not user-facing in Unreleased | P0 for next release |
| GAP_ANALYSIS.md competitor table (stars, "stellar-cli has no XDR decode") | Dated 2026-07-31; **not re-verified** this pass | P1 if cited in appeal |
| Rapid 0.1.0 → 2.5.0 in ~8 days | 14 GitHub releases 2026-08-04..08 | P0 optics vs Wave "past repo activity" |

## Maintainer activity

- Human authors in `git shortlog -sn --all`: YUSEP MAULANA 170, naninu123 16, sabo 14, dependabot 3. GitHub contributors API: `naninu123` 184, dependabot 2. **One human.**
- Commit calendar: burst 2026-08-04 (26), 08-05 (47), 08-06 (36), 08-07 (35), 08-08 (20), then 08-12 (8), 08-13 (1), 08-14 (9), 08-15 (1). **Silence 08-09..08-11 and 08-16..08-19.**
- GitHub user `naninu123`: 103 public repos, 8 followers, created 2022-04-06. Stellar/Soroban-named repos are **forks** (`stellarview-tui`, `Grainlify-Stellar-Contracts`, `soroban-security-portal`, `Stellar-forge`, `Soroban-Contract-Explorer`, …). Cite as prior ecosystem **exposure**, not original maintained products, not sdkt adoption.
- Org `SaboLabs`: created 2026-06-22, description "Independent security research & automation…", 1 real product repo (`soroban-devkit`) plus GH demo stubs.

## External adoption — VERIFIED ABSENT (method + date)

Checked 2026-08-19:

| Probe | Result |
|---|---|
| crates.io max_version | all 8 crates `2.5.0` |
| crates.io downloads (cumulative) | core 133, xdr 113, wasm 113, rpc 107, storage 95, audit 81, cli 76, example-rule 58. **Downloads ≠ users.** |
| reverse_dependencies | every `crate_id` is a workspace sibling (or self). **0 external crate dependents.** |
| GitHub quoted search `"SaboLabs/soroban-devkit"` | total_count **1** (self) |
| GitHub code search `SaboLabs/soroban-devkit filename:Cargo.toml` (auth `gh`) | total **0** |
| Stars | 2: `naninu123` (self), `Shadow-MMN` |
| Forks | 1: `Shadow-MMN/soroban-devkit` (0 stars, last push 2026-08-08, no independent commits claimed) |
| Discussions | enabled, **0** threads |
| Open issues | **0** (counter=1 is Dependabot PR #27) |
| Closed issues | 7, all self-filed ENG-* tickets |
| Good first issues | **0** |
| Discord / Stellar Discord / StackExchange / X links in public docs | **ABSENT** |

Name collision: `sorocore/soroban-devkit` is a different TypeScript project — FALSE POSITIVE, not this repo.

## Community / ecosystem evidence

ABSENT as *interaction*. Present as *surfaces*: Discussions enabled, issue templates (bug/feature/good-first), SECURITY.md private advisory, SUPPORT.md, website Community CTA pointing at GitHub issues.

Compatibility CI against `stellar/soroban-examples` is **self-run validation**, not external usage.

## Release evidence

- 14 published GitHub Releases (v0.6.0-alpha through v2.5.0). Cadence is extremely dense (multiple tags per day 2026-08-05..08-07).
- v2.5.0 assets: 3 tarballs (no Windows, no linux-aarch64). crates.io `sdkt-cli` versions: 0.6.0-alpha, 2.0.0, 2.4.0, 2.5.0 (republished 2026-08-14).
- **Shipped on main, not in any GitHub Release:** M40 local plugin store, M41 on-chain inspect, M42 on-chain upgrade-safety, M43 live ABI events, M44 live ABI storage, Web Playground, landing page, Windows release-workflow target, several live-RPC compatibility fixes, M45–M47 docs/hygiene, M46 Windows path tests.
- That is a **real** next-release justification, not a vanity bump — **if** CHANGELOG/theme is honest.

## Current positioning

README + website: "unified, offline-capable toolkit for Stellar / Soroban development" covering inspect / decode / audit / diff / deploy. Accurate vs CLI surface. Missing: Playground as first-touch; no "used by" claims (good). `docs/scf.md` already has an honest traction section.

## Strongest assets

1. Source-verified Soroban depth (XDR, ContractSpec, upgrade-safety, storage TTL, audit rules, live RPC inspect/events/storage).
2. Test + CI density (3 OS, clippy -D warnings, compatibility vs official examples, install.sh selftest).
3. Distribution: crates.io + GitHub Releases + install.sh + live Pages playground.
4. Honest `docs/scf.md` traction block (do not dilute).
5. Post-tag real work (M40–M44 + playground) that a reviewer can demo today on `main` / github.io even though it is not in v2.5.0.

## Highest-impact gaps (for Wave appeal)

Wave rejection cited: past repo & hackathon activity, code/docs substance, maintainer activity in the ecosystem, overall relevance.

| ID | Gap | Class |
|---|---|---|
| G1 | Repo is ~16–19 days old with a compressed 0.1→2.5 version ladder — looks like a sprint/hackathon artifact, not a lived-in tool | P0 |
| G2 | Zero external developer interaction (issues, discussions, dependents, independent PRs) | P0 |
| G3 | Latest **published** release omits the most reviewer-visible Soroban work (M40–M44, playground, Windows) | P0 |
| G4 | README/quick-start do not send a stranger to the live Playground in <30s | P0 |
| G5 | Docs claim a Windows release asset that v2.5.0 does not ship | P0 |
| G6 | No presence in Stellar Discord / Dev Discord / StackExchange / stellar.org forums | P1 |
| G7 | Maintainer Stellar history is mostly **forks**, not original maintained tooling besides sdkt | P1 |
| G8 | Single-maintainer, all historical issues are self-ENG tickets | P1 |
| G9 | No good-first-issue, 0 discussion posts, SUPPORT example stale | P2 |
| G10 | Competitive table in GAP_ANALYSIS unverified vs current `stellar-cli` | P2 |

Adding features will not close G1/G2. Shipping a truthful v2.6.0 (or 2.5.1) + real external conversation + 14 days of visible, non-burst activity will.
