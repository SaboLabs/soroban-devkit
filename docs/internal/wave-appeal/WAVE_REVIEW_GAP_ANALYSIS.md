# WAVE_REVIEW_GAP_ANALYSIS.md

Role: skeptical Stellar Wave reviewer discovering SaboLabs/soroban-devkit on 2026-08-19.
Question: "What evidence would convince me this is an **active, relevant Soroban developer-tooling project**, not merely a technically large GitHub repository produced in a two-week burst?"

Evidence classes: VERIFIED / INFERENCE / ABSENT / UNKNOWN.

## Reviewer first 10 minutes

1. GitHub created **2026-08-03**. First commit **2026-07-31**. Age ≈ 2.5 weeks.
2. Tags: v0.1.0 … v2.5.0 in eight days. That pattern scores as **hackathon/activity packing**, which is explicitly in the rejection criteria ("past repo & hackathon activity").
3. Stars 2 (one is the owner). Fork 1. Discussions 0. Open issues 0. Contributors = owner + Dependabot.
4. Then the reviewer opens README / website / playground / `sdkt --help` / crates.io.

Substance after minute 10 is **real**. Optics in minute 1–3 currently lose the appeal.

## A. Stellar / Soroban relevance

| Signal | Class | Note |
|---|---|---|
| CLI named and described as Soroban toolkit | VERIFIED | clap about, README, crates.io keywords |
| XDR decode `ScVal` / `TransactionEnvelope` / `ContractEvent` | VERIFIED | `sdkt-xdr`, `sdkt decode` |
| ContractSpec / WASM inspect | VERIFIED | `sdkt-wasm::parse_contract_spec`, `sdkt wasm inspect` |
| Upgrade-safety / SpecDiff | VERIFIED | `UpgradeVerdict`, `sdkt diff --upgrade-safety`, on-chain `verify --upgrade-safety` (M42, on main not in tag) |
| Storage TTL Instance/Persistent/Temporary | VERIFIED | `sdkt-storage` |
| Static AUTH/MOVE rules on Soroban source | VERIFIED | `sdkt audit`, AUTH-001 tests |
| RPC inspect / events / health / deploy | VERIFIED | `sdkt-rpc` |
| CI vs `stellar/soroban-examples` | VERIFIED | `.github/workflows/compatibility.yml` last success 2026-08-15 |
| Default testnet passphrase | VERIFIED | README |

**Reviewer verdict:** relevance of *code* is not the failure mode. Relevance of *the project as a lived ecosystem artifact* is.

## B. Developer usefulness

Playground (browser WASM inspect, local-only) + offline `wasm inspect` / `audit` / `diff` is a real 5-minute path **if found**. README does not lead with Playground. Quick-start inspects a 198-byte fixture, not a contract the visitor brought. That is useful but not "I felt this on *my* contract in 60 seconds" unless they hit github.io/playground.

## C. Maintainer activity

Dense 2026-08-04..08-08, then thin. Last origin push 2026-08-15. Local unpushed SCF docs 2026-08-15. Four-day gap before this audit. One human. Prior Stellar GitHub activity is mostly forks.

Wave criterion "maintainer activity in the relevant ecosystem" is **not satisfied** by commit volume inside this repo alone.

## D. External usage

VERIFIED ABSENT: 0 reverse-deps, 0 code-search Cargo.toml hits, 0 discussions, 0 external issues, 0 independent PRs. crates.io downloads are distribution noise (dozens).

## E. Community activity

Surfaces exist (Discussions, templates, website CTA). Activity ABSENT.

## F. Documentation

Volume and honesty (`docs/scf.md`) are strong. Two trust-killers: Windows asset listed but missing from the current release; Playground/live site under-linked from README.

## G. Releases

Many tags, too fast. Latest tag **lags main** by the work a Wave reviewer would actually demo (playground, M40–M44). That is the opposite of "shipping evidence."

## H. Contribution pathway

CONTRIBUTING.md + templates + CoC exist. No open starter issues. No external PR history except Dependabot. Pathway is theoretical.

## I. Evidence the project is actually used

ABSENT. Compatibility CI is the closest thing; it is the project's own workflow.

## What would change a skeptical "no"

Ranked by expected impact on **appeal success** (not on code quality):

| Rank | Gap | Why a Wave reviewer cares | Fix type |
|---|---|---|---|
| 1 | Age + version-ladder optics (G1) | Matches the written rejection axis | Time + one honest release, **not** more tags |
| 2 | Zero external interaction (G2) | "Is anyone else here?" | Real outreach / discussion / independent issue replies |
| 3 | Published release lags demoable work (G3) | Reviewer installs v2.5.0 and misses playground/M40–M44 | One justified release after CHANGELOG |
| 4 | Playground not on the README critical path (G4) | First-touch value is already built | Docs/link |
| 5 | False Windows binary claim (G5) | Instant credibility fail | Docs now; asset on next tag |
| 6 | No Stellar-community footprint (G6) | "Maintainer in the ecosystem" | One technical post + one Discord thread, no spam |
| 7 | Maintainer story is forks + this burst repo (G7) | Past activity criterion | Point at real prior work honestly; don't invent |
| 8 | Empty contribution graph (G8/H) | Looks closed | Real good-first **only if** a real task exists |

**Do not:** add features, mint tags without user-visible delta, fake issues/stars, rewrite architecture, submit the appeal this week.

## Positioning (accurate)

Keep: *A developer toolkit for inspecting, analyzing, validating, and safely shipping Soroban smart contracts — offline-first CLI + in-browser WASM inspector.*

Do not claim "production-grade" as social proof. The architecture is production-*minded*; adoption is not production.
