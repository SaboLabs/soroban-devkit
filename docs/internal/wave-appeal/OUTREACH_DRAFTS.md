# OUTREACH_DRAFTS.md

**Status of every item below: NOT SENT — REQUIRES USER AUTHORIZATION**

No `gh issue create`, no Discussion post, no Discord message was sent in this session.

## 1. GitHub Discussion draft (this repo)

Suggested category: Ideas or Q&A
Title: `Feedback wanted: offline Soroban inspect / audit / upgrade-safety CLI`

Body:

```
I maintain sdkt (https://github.com/SaboLabs/soroban-devkit), an offline
Soroban CLI: WASM/ContractSpec inspect, static AUTH/MOVE analysis, and
upgrade-safety diffs. There is also an in-browser inspector that never
uploads the contract:

  https://sabolabs.github.io/soroban-devkit/playground/

Install (Rust 1.88+):

  cargo install sdkt-cli
  sdkt --version          # sdkt 2.5.0
  sdkt wasm inspect path/to/contract.wasm
  sdkt audit path/to/contracts/foo/src/lib.rs
  sdkt diff --old-wasm old.wasm --new-wasm new.wasm --upgrade-safety

I am looking for technical feedback from people who ship Soroban contracts:

- Did inspect/audit/diff produce something you would actually put in CI?
- What broke or confused you in the first ten minutes?
- False positives you hit (especially AUTH-001 on getters/helpers) are
  useful; please paste command + output.

Not asking for stars, endorsements, or adoption. Issues and this thread
are the right place. SECURITY.md for anything that is actually a sdkt vuln.
```

## 2. Stellar developer Discord message

Channel: a **dev** / tooling channel only (not general/announcements). One message, no cross-post.

```
Posted a short ask for technical feedback on sdkt, an offline Soroban
inspect/audit/upgrade-safety CLI + in-browser WASM inspector (bytes stay
in the tab): https://sabolabs.github.io/soroban-devkit/playground/

  cargo install sdkt-cli
  sdkt wasm inspect <contract.wasm>
  sdkt audit <contracts/*/src/lib.rs>

Discussion on the repo if you try it and it breaks or is noisy:
https://github.com/SaboLabs/soroban-devkit/discussions
(that thread is not posted until authorized). Not a launch/star ask.
```

Do not send until the Discussion URL exists, then paste the real link.

## 3. External GitHub issue drafts

**None.** Local audits of five public contract repos produced **no**
maintainer-useful findings (AUTH-001/MOVE-001 were false positives or
non-entrypoints). Per rules: do not open an issue merely because the tool
emits output.

If later a verified missing `require_auth()` on a **public mutating
entrypoint** appears, use this template (still NOT SENT until authorized):

```
Title: Optional: sdkt audit note on <exact fn> (tool finding, not a vuln report)

Hi — I maintain sdkt (https://github.com/SaboLabs/soroban-devkit),
`cargo install sdkt-cli`. Read-only local run on current default branch:

  sdkt audit <exact-path>

Output (trimmed):
  <paste exact AUTH-001 line + surrounding function>

I checked the function body: <one sentence: require_auth missing on this
pub entry / OR auth only in helper X>. This is a *tool finding* for your
review, not a claim of exploitability. Close if it is noise.

Not asking you to depend on sdkt. No grant/Wave ask.
```

## Send checklist (operator)

- [ ] Authorize Discussion
- [ ] Authorize Discord (after Discussion URL)
- [ ] Authorize any future issue only if `EXTERNAL_AUDIT_*.md` marks Actionable=yes
