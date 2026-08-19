# RELEASE_PROPOSAL.md

**Do not tag, push, or `cargo publish` from this document.** Operator must authorize.

## VERSION

Candidate: **v2.6.0** (minor). Not 2.5.1.

Why not 2.5.1: user-visible capabilities landed (plugin store, on-chain inspect/upgrade-safety/events/storage ABI, Web Playground, website, Windows release target, live-RPC compat fixes). That is minor, not patch.

Why not 3.0.0: no announced breaking CLI removals/renames (`docs/scf.md`).

Why not "cut a tag just for Wave": the work already exists on `main`. A tag is justified **iff** CHANGELOG `[Unreleased]` is rewritten to list that work and Windows/linux-aarch64 assets actually upload.

## THEME

"On-chain inspect + in-browser playground + Windows binary — the v2.5.0 CLI plus the Soroban workflows a reviewer can demo without cloning."

## USER-VISIBLE CHANGES (from `git log v2.5.0..origin/main`, plus local HEAD)

Must be rewritten into CHANGELOG before any tag:

- M40 local audit plugin store (`sdkt plugin list/show/install/remove/update`)
- M41 on-chain WASM/contract inspection (`sdkt wasm metadata --contract`)
- M42 on-chain vs local upgrade-safety (`sdkt verify --upgrade-safety`)
- M43 live-contract ABI for events
- M44 on-chain ABI for storage decode
- Live RPC LedgerEntry / TTL / M41 / M43 compatibility fixes
- Web Playground (browser ContractSpec inspector, local-only)
- Landing page (GitHub Pages)
- Windows x86_64 in `release.yml` (asset will exist **only if this tag's workflow runs**)
- Windows path tests (M46), contributor-root hygiene (M47)
- Ownership URLs naninu123 → SaboLabs
- (If 6babff8 is pushed) committed examples + onboarding-smoke CI

`CHANGELOG.md` `[Unreleased]` today only mentions removing unused `sdkt-wasm → sdkt-core`. That is **not** an honest 2.6.0 notes file.

## SOROBAN VALUE

A developer can: drop a WASM in the browser; inspect ContractSpec offline; audit source; diff upgrades; talk to testnet inspect/events/storage with on-chain ABI — without five CLIs.

## MIGRATION NOTES

None expected for CLI flags. Document: crates.io `sdkt-cli` 2.5.0 remains installable until publish; Playground is Pages-only (not a crate).

## TEST EVIDENCE (required before tag, not yet run as a release gate this pass)

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace`
- `bash scripts/smoke_examples.sh` (needs 6babff8 on the tagged commit)
- compatibility workflow green
- After tag: confirm **four** release assets including `sdkt-x86_64-pc-windows-msvc.zip`

## INSTALLATION (post-tag)

```
curl -fsSL https://raw.githubusercontent.com/SaboLabs/soroban-devkit/main/install.sh | bash
cargo install sdkt-cli --version 2.6.0
```

Windows: unzip the msvc zip **or** `cargo install sdkt-cli`.

## EXAMPLES

Keep: `sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_old.wasm`, `sdkt audit examples/sample_token/src/lib.rs`, playground URL.

## BLOCKERS BEFORE PUBLISH

1. Push or include `6babff8` so examples/smoke exist on the tagged SHA.
2. Rewrite CHANGELOG Unreleased → `[v2.6.0]`.
3. Fix README Windows claim so it matches assets **after** the workflow succeeds (or don't list Windows until the asset exists).
4. Operator authorization for tag + GitHub Release + crates.io publish.

## Decision (2026-08-19 validation pass)

**NO RELEASE YET.**

User-visible work on `main` after v2.5.0 is real (M40–M44, playground, website, Windows *workflow*, RPC fixes). That *would* justify **v2.6.0** as a theme.

Blockers that keep the decision at NO:

| Requirement | Status |
|---|---|
| Meaningful user-visible changes exist on main | YES (unreleased) |
| Accurate CHANGELOG `[Unreleased]` covering those changes | **NO** — still only `sdkt-wasm` unused-dep note |
| Verified GitHub Release assets including Windows zip | **NO** — v2.5.0 has linux+mac only; zip exists only after a future tag's `release.yml` |
| Verified installation path for the *new* version | **NO** — cannot verify 2.6.0 install until tag+publish |
| This session validation (fmt/test/clippy) | YES on current HEAD |
| Playground availability | YES (Pages 200) |
| No stale v2.5.0 claims | Public docs in **working tree** now honest; **origin/main** still has the old Windows-zip claim until a commit is authorized |

Cutting a tag now would ship a CHANGELOG that under-describes the release and still requires operator authorization (forbidden this pass).

Next release action (when operator asks): rewrite CHANGELOG → include `6babff8` + docs honesty → tag v2.6.0 → confirm four assets → then crates.io. Not before.
