# WAVE_APPEAL_EVIDENCE.md

# Stellar Wave Appeal Evidence

## Project

SaboLabs/soroban-devkit — MIT-licensed Rust CLI + crates for Stellar/Soroban inspect, decode, audit, diff, deploy.

- GitHub: https://github.com/SaboLabs/soroban-devkit
- Pages: https://sabolabs.github.io/soroban-devkit/
- Playground: https://sabolabs.github.io/soroban-devkit/playground/
- crates.io binary crate: `sdkt-cli` (installs `sdkt`)

Snapshot date: **2026-08-19**. Local HEAD `6babff88` (unpushed). origin/main `3f7d3956`.

## Current Release

- GitHub Release **v2.5.0** published 2026-08-08: https://github.com/SaboLabs/soroban-devkit/releases/tag/v2.5.0
- Assets: `sdkt-x86_64-unknown-linux-gnu.tar.gz`, `sdkt-x86_64-apple-darwin.tar.gz`, `sdkt-aarch64-apple-darwin.tar.gz`
- Windows zip: **NOT YET ESTABLISHED** on this tag (workflow on main includes windows-latest; tag predates that commit)
- crates.io `sdkt-cli` max 2.5.0, versions 0.6.0-alpha / 2.0.0 / 2.4.0 / 2.5.0 (2.5.0 crate published 2026-08-14)

## Technical Substance

VERIFIED: 8 published crates + playground glue; ~25k LOC Rust; 482 test attributes; CLI surface as `sdkt --help`; CI matrix 3 OS + clippy -D warnings + MSRV 1.88.0; compatibility workflow vs `stellar/soroban-examples`; last CI success https://github.com/SaboLabs/soroban-devkit/actions/runs/31856606475

## Soroban Relevance

VERIFIED: see `SOROBAN_RELEVANCE_MATRIX.md`. Code paths for XDR, ContractSpec, upgrade-safety, storage TTL, AUTH rules, RPC inspect/events, `stellar/soroban-examples` CI.

## Maintainer Activity

VERIFIED: single human (`naninu123` / YUSEP MAULANA / sabo), 184 GitHub contributions on this repo. Burst 2026-08-04..08-08, last origin push 2026-08-15. Org SaboLabs created 2026-06-22.

Prior Stellar-named GitHub repos under the maintainer are **forks** (stellarview-tui, Grainlify-Stellar-Contracts, soroban-security-portal, Stellar-forge, Soroban-Contract-Explorer, …). Cite as ecosystem exposure only.

Independent Stellar org affiliation / employment: **NOT YET ESTABLISHED** (none claimed).

## Documentation

VERIFIED: README, docs/{quick-start,examples,installation,cli,ci-cd,compatibility,plugin-authoring,scf}, CONTRIBUTING, SECURITY, SUPPORT, CHANGELOG, ROADMAP.

Known inaccuracies at snapshot: Windows release asset listed but missing; Playground not in README; SUPPORT example 2.4.0; CHANGELOG Unreleased incomplete vs main.

## Developer Experience

VERIFIED: install.sh, crates.io `cargo install sdkt-cli`, source build, quick-start offline inspect/audit/diff, committed examples **on local HEAD only**.

Live first-touch playground: VERIFIED HTTP 200.

End-to-end walk recorded this sprint: **NOT YET ESTABLISHED** (plan days 3–5).

## Playground

VERIFIED: `website/playground/` + `crates/sdkt-playground`; live URL 200; local-only inspect (no wallet). Analyze/XDR/Diff/Health nav items labeled coming soon.

## External Developer Feedback

**NOT YET ESTABLISHED.** Discussions totalCount 0. External issues opened by others: 0.

Plan: `EXTERNAL_FEEDBACK_PLAN.md`. Drafts (unsent): `OUTREACH_DRAFTS.md`.
Local evaluations: `EXTERNAL_AUDIT_2026-08-19.md` (5 repos, 0 issues opened).

## Adoption evidence model (do not collapse)

Snapshot 2026-08-19. Nothing in this table is upgraded without a URL.

| Class | Meaning | Current |
|---|---|---|
| **OUTREACH** | We contacted someone / posted | **0**. Drafts exist, **NOT SENT**. |
| **ENGAGEMENT** | External person replies / interacts | **NOT ESTABLISHED** |
| **USAGE** | External person actually ran sdkt | **NOT ESTABLISHED** (local runs in `/tmp/sdkt-probe` are *our* runs) |
| **FEEDBACK** | External person gave technical feedback | **NOT ESTABLISHED** |
| **ADOPTION** | Sustained/repeated external usage (dep, CI Action, confirmed) | **NOT ESTABLISHED** |

crates.io downloads remain **distribution**, not USAGE or ADOPTION.
Self-stars / own fork / Dependabot / self-ENG issues do not count.

## Community / Ecosystem Activity

**NOT YET ESTABLISHED** beyond self-run CI and self-owned GitHub surfaces. No Discord/StackExchange/stellar.org links in-repo. Discord draft unsent.

## Releases

VERIFIED: 14 GitHub releases in ~5 days (2026-08-04..08-08) then none. Main is **ahead** of v2.5.0 by M40–M44, playground, website, Windows workflow, RPC fixes. Next release proposal: `RELEASE_PROPOSAL.md` (not published).

## Testing / Reliability

VERIFIED as of last GH Actions on origin/main (success). Local `cargo test --workspace` **not re-executed** in Phase 0 (binary `--help`/`--version` were). Do not claim a local full suite pass from this file.

## Cross-platform Support

VERIFIED: CI test job matrix ubuntu-latest, macos-latest, windows-latest. Release **assets** for v2.5.0: linux-x64, darwin-x64, darwin-arm64. Windows published binary: **NOT YET ESTABLISHED**. linux-aarch64 release asset: **NOT YET ESTABLISHED** (`install.sh` mentions the target).

## Material Improvements Since Initial Review

UNKNOWN exact Wave-review date. Observable after 2026-08-08 tag (likely overlapping/after a Wave look): M40–M44, live RPC fixes, Pages landing, Playground, Windows release target, M45–M47 docs/hygiene, SCF-honesty section in `docs/scf.md`, local examples/smoke (unpushed).

Repo **created 2026-08-03** — "past repo activity" cannot be backfilled. Only forward activity + honest narrative.

## Evidence Links

- Repo: https://github.com/SaboLabs/soroban-devkit
- Release v2.5.0: https://github.com/SaboLabs/soroban-devkit/releases/tag/v2.5.0
- Pages: https://sabolabs.github.io/soroban-devkit/
- Playground: https://sabolabs.github.io/soroban-devkit/playground/
- crates.io: https://crates.io/crates/sdkt-cli
- CI: https://github.com/SaboLabs/soroban-devkit/actions/workflows/ci.yml
- Compatibility: https://github.com/SaboLabs/soroban-devkit/actions/workflows/compatibility.yml
- Honest traction: `docs/scf.md` § Honest current traction (refresh numbers on appeal day)

External dependents / third-party writeups / Discord threads: **NOT YET ESTABLISHED**.
