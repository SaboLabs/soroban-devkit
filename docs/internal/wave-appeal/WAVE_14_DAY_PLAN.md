# WAVE_14_DAY_PLAN.md

Clock starts when the operator says the waiting period is running. Do not submit the appeal in this window.

Optimize for: real development, real developer value, real Stellar relevance, real external evidence, auditable proof. Skip anything that only grows the commit graph.

## DAYS 1–2 — Baseline + positioning

| TASK | WHY | EXPECTED EVIDENCE | DEPS | RISK | DONE CONDITION |
|---|---|---|---|---|---|
| Freeze baseline SHAs / crates.io / GH counts (this folder) | Appeal later must not mix dates | This file set dated 2026-08-19 | none | stale numbers | Reports cite HEAD + API dates |
| README: Playground + Pages URL on the critical path | Reviewer minute-1 | Diff in README | none | overclaim | Link 200s; no user counts added |
| README/CONTRIBUTING/quick-start: honest Windows asset status | Credibility | Docs match `gh release view v2.5.0` | none | still wrong after 2.6.0 | Either "not in v2.5.0" or listed only once the zip exists |
| SUPPORT.md example version 2.5.0 | Hygiene | one-line | none | none | grep 2.4.0 gone from SUPPORT |
| Do **not** invent features / tags | Wave optics | empty | — | temptation | no extra tags |

## DAYS 3–5 — DX + legitimate external feedback

| TASK | WHY | EXPECTED EVIDENCE | DEPS | RISK | DONE CONDITION |
|---|---|---|---|---|---|
| Walk the new-user path once (Pages → playground → install → wasm inspect → audit) | Prove DX, note friction | notes in WAVE_APPEAL_EVIDENCE | live Pages | Pages cache | Path documented with URLs |
| Push `6babff8` **if operator authorizes** so origin matches local examples/smoke | Reviewer cloning main sees examples | origin/main == local | operator | leaking unreviewed SCF docs | origin has smoke job green |
| GitHub Discussion: feedback wanted (commands + playground) | First-party conversation surface | Discussion URL | none | zero replies | Post exists; not counted as adoption |
| Local-audit 5 external Soroban repos; open ≤5 technical issues | Real outreach | issue URLs + local command output | `sdkt` binary | looking like spam | 5 audits logged; ≤5 issues; no Wave pitch |
| Respond to any reply <24h | Turns OUTREACH into INTEREST | comment timestamps | replies | silence | every reply answered or classified NO RESPONSE |

## DAYS 5–7 — Stellar ecosystem activity

| TASK | WHY | EXPECTED EVIDENCE | DEPS | RISK | DONE CONDITION |
|---|---|---|---|---|---|
| One Stellar Dev Discord message (dev channel, one time) | Maintainer-in-ecosystem criterion | timestamped link/screenshot | Discussion URL to point at | spam / off-channel | 1 message, 0 repeats |
| Optional: answer 1 existing StackExchange/Discord question with a command | Usefulness not launch | URL | a real question | off-topic pitch | Answer solves *their* question |
| Do not post in multiple channels the same day | anti-spam | — | — | ban | 1 ecosystem post |

## DAYS 7–9 — Playground / demo adoption

| TASK | WHY | EXPECTED EVIDENCE | DEPS | RISK | DONE CONDITION |
|---|---|---|---|---|---|
| Record a 3–5 min demo: playground inspect + CLI audit/diff on a **public** testnet or fixture WASM | Reviewer who will not install | unlisted video or `docs/scf-demo.md` update with real outputs | playground live | fake output | Commands in the demo re-run locally match |
| If playground JS bugs appear while recording, smallest fix only | Real quality | PR/commit with test | none | scope creep | fix + existing playground tests |

## DAYS 9–11 — Release / shipping evidence

| TASK | WHY | EXPECTED EVIDENCE | DEPS | RISK | DONE CONDITION |
|---|---|---|---|---|---|
| Draft CHANGELOG 2.6.0 from `v2.5.0..HEAD` | Honest shipping | CHANGELOG section | freeze feature scope | missing M40–M44 | User-visible list complete |
| Operator-authorized tag v2.6.0 | Close G3 | GitHub Release + Windows zip + crates.io 2.6.0 | CI green, CHANGELOG | tag without Windows job success | 4 assets; `sdkt --version` 2.6.0 from install.sh |
| If operator does **not** authorize tag | still OK | leave proposal | — | cutting a vanity tag | no tag |

## DAYS 11–12 — Evidence collection

| TASK | WHY | EXPECTED EVIDENCE | DEPS | RISK | DONE CONDITION |
|---|---|---|---|---|---|
| Refresh WAVE_APPEAL_EVIDENCE.md with new URLs/counts | Appeal packet | dated API pulls | days 3–11 | inflating class | every row sourced |
| Re-run crates.io reverse-deps + GH search | catch real adoption | same probes as baseline | none | false positives | collisions excluded |

## DAYS 13–14 — Appeal preparation (draft only)

| TASK | WHY | EXPECTED EVIDENCE | DEPS | RISK | DONE CONDITION |
|---|---|---|---|---|---|
| Draft appeal narrative mapping to rejection axes | past activity / substance / maintainer / relevance | private draft, not submitted | evidence file | submitting early | draft cites only VERIFIED rows |
| List remaining NOT YET ESTABLISHED | honesty | same | — | filling gaps with hope | ABSENT stays ABSENT |

## Explicitly not scheduled

More milestones, more tags in the 0.x/2.x burst style, fake good-first issues, star exchanges, README user counts, architecture rewrites, Wave-channel lobbying.
