# EXTERNAL_FEEDBACK_PLAN.md

Goal: **real** Soroban-developer interaction that can be cited in a Wave appeal.
Not vanity metrics. No fake accounts, no star-begging, no spam.

Classification ladder (never skip):
OUTREACH → INTEREST → VALIDATION → ADOPTION
Opened issue ≠ adoption. Star ≠ adoption. Download ≠ user.

## Target audience

1. Rust Soroban contract authors with public `contracts/**/src/lib.rs`.
2. Teams already using `stellar-cli` who also need inspect/diff/audit.
3. Security-minded maintainers (upgrade-safety, AUTH checks).
4. Stellar Dev Discord / Discord "dev-discussion" / StackExchange stellar tag readers.

Exclude: own repos, forks of sdkt, abandoned repos, `sorocore/soroban-devkit` name collision, marketing-only issues.

## What to ask (one ask per channel)

Lowest-friction offer (offline, no secrets):

```
cargo install sdkt-cli
sdkt wasm inspect path/to/contract.wasm
sdkt audit path/to/contracts/*/src/lib.rs
```

Or: open https://sabolabs.github.io/soroban-devkit/playground/ and drop a WASM.

Ask only:
- Did inspect/audit/diff produce something you could use in CI?
- What broke / what was confusing in the first 10 minutes?
- Optional: would you want `sdkt audit` as a GitHub Action on PRs?

Do **not** ask for stars, Wave support, testimonials, or "please adopt."

## Where to ask

| Channel | How | Cap | Citable evidence if they reply |
|---|---|---|---|
| GitHub Discussions on **this** repo | One "Feedback wanted" post with exact commands + playground URL | 1 post | Thread URL + third-party replies |
| GitHub issues on **external** contract repos | One technical issue per repo, after a **local** `sdkt audit`/`wasm inspect` against their public source | max 5 repos, 1 issue each, no follow-up unless they answer | Issue URL; classify OUTREACH until they respond |
| Stellar Dev Discord | One message in an appropriate **dev** channel with playground + `cargo install sdkt-cli`, asking for breakage reports | 1 message, no repeats | Message link / screenshot with timestamp (no scrape-spam) |
| Stellar StackExchange | Only if a real unanswered tooling question exists that sdkt answers — answer the question, mention the command, don't dump a pitch | 0–1 | Answer URL |
| Direct maintainer outreach | Only for repos where local run produced a **reproducible** finding or a clean report | same as issues | email/GH only if they have a listed contact |

Hard limits: no Telegram spam, no Discord cross-posting, no "please star", no SCF/Wave mention in outreach (Wave is not their problem).

## Suggested issue body (adapt numbers to the actual local run)

```
Hi — I maintain `sdkt` (https://github.com/SaboLabs/soroban-devkit), an offline
Soroban inspect/audit/diff CLI (`cargo install sdkt-cli`).

I ran it read-only against this repo's public contract sources:

  sdkt audit <exact-path>
  # result: <paste exact summary, e.g. "AUTH-001 on admin_set at src/lib.rs:…">
  # or: 0 findings

This is a tool finding, not a vulnerability report. Repro is the command above.
If useful, there is also a GitHub Action wrapper for `sdkt audit` +
`sdkt diff --upgrade-safety`. Not asking you to depend on it — flagging in
case the output is helpful. Happy to close this if it's noise.
```

Run the command **before** opening the issue. If the clone/audit fails, skip that target.

## Expected useful feedback

- "Command X panicked on our WASM" → real bug → fix → cite the issue.
- "We already use stellar-cli for this" → competitive intel, not a loss.
- Silence → NO RESPONSE. Leave it. Do not re-ping.

## What may be cited in an appeal

Allowed:
- URLs of third-party issues/discussions **and** the reply text.
- "Opened N evaluation issues on date D; M replies; K reproduced locally."
- crates.io download counts labeled as distribution, not users.

Forbidden:
- "used by the ecosystem"
- counting own stars/forks/self-issues
- counting Dependabot as community

## 14-day cadence

Days 3–5: local-audit 5 live Soroban repos; open ≤5 issues; post 1 Discussion.
Days 5–7: one Discord dev message after Discussion exists (so there is a place to send people).
Days 7–14: respond to every reply within 24h; if a real defect appears, fix on a branch (no vanity commits).
