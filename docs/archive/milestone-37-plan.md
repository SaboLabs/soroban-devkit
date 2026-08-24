# Milestone 37 — Dependency Version Resolution

> **STATUS: SCHEDULED — scope defined, not yet implemented.**
> Promoted to an official roadmap milestone (see `ROADMAP.md` §4 "Package Manager & Distribution").
> No Rust code has been written yet. This document is the implementation contract.

**Branch (when work begins):** `feat/milestone-37`
**Baseline:** M36.0 merged to `main` (package loop `validate → fetch → update → verify` complete).
**Target:** Add version-constraint resolution to the package manager so `sdkt package update`
can honor semver ranges, compare resolved commits, and surface update availability per constraint.
**Status:** Documented scope — not started.

---

## 1. Objectives

Extend the M35/M36 package manager with **version resolution** semantics on top of the
already-locked `commit_sha` / `resolved_reference` model:

1. Allow `[dependencies.<name>]` to declare an optional **version constraint** (`version = "1.2"`,
   `">=1.0, <2"`, `branch = "main"` already supported for git) for git tags and registry sources.
2. Resolve the *best available* version/commit that satisfies the constraint via `git ls-remote`
   (offline for local remotes; contacts only the declared git remote when a network ref is needed) —
   reusing the `resolve_available_commit` primitive from `sdkt-core::sync`.
3. Compare the resolved best version against the locked `commit_sha` / tag and report
   `up-to-date` / `update-available` / `constraint-unsatisfied` in `sdkt package update --check`.
4. Keep the existing `rev` (immutable pin) and `path` (local) behaviors exactly as M36.0 defines them.

---

## 2. Scope

### In Scope
- New optional `version` field on `Dependency` (semver-shaped), additive and non-breaking.
- A `VersionResolver` (in `sdkt-core::package` or `sdkt-core::sync`) that, given a constraint and a
  remote's tag list, selects the highest satisfying tag/commit — reusing `git_cache_key`,
  `git_bin`, and `resolve_available_commit`.
- `sdkt package update --check` / `--dry-run` output enriched with constraint satisfaction state.
- Unit tests: constraint parse, best-version selection, pinned-rev bypass, unsatisfied constraint,
  offline local-remote resolution.
- CLI integration tests: offline update with a semver constraint against a local git repo.
- Docs: README, CHANGELOG (`[Unreleased]`), `docs/cli.md`, this plan.

### Out of Scope (this milestone)
- A remote **registry** / index server (deferred — `DependencyFetcher` already abstracts acquisition).
- Automatic `Cargo.toml` / `Cargo.lock` cross-resolution for Rust deps (out of scope).
- Downgrade / rollback commands (only forward update is in scope; `rev` stays immutable).
- Publishing flows (that is M38).

---

## 3. Architecture

```
sdkt-cli (package update)
   │  routes + formats (reuses OutputFormat)
   ▼
sdkt-core::sync  (plan_updates — extended to call VersionResolver)
   │  reuses resolve_available_commit, git_cache_key, git_bin
   ▼
sdkt-core::package (VersionResolver — NEW, pure logic)
   │  semver constraint match over remote tag list
   ▼
sdkt-core::lock   (DependencyLock — unchanged fields; version recorded alongside commit_sha)
```

- `VersionResolver` is **pure** (no I/O): it takes a `Vec<(tag, commit)>` (already fetched via
  `git ls-remote`) and a constraint, and returns the best match. All git I/O stays in `sync.rs` /
  `fetch.rs`. This avoids duplicating the ls-remote / cache-key logic.
- No new dependency-graph, topo-sort, validation, or hashing code — all reused from M35/M36.

---

## 4. Deliverables

- `Dependency.version: Option<String>` (semver string) added with `#[serde(default)]`.
- `VersionResolver::best_match(tags: &[(String, String)], constraint: &str) -> Option<(String, String)>`.
- `plan_updates` consults `VersionResolver` when a `version` constraint is present; `rev`/`path` deps
  bypass it (unchanged M36.0 behavior).
- Enriched `UpdateChange.detail` strings for `constraint-unsatisfied` / `update-available (vX.Y.Z)`.
- Unit + CLI integration tests (offline, local git remotes — no network).
- Documentation updates only.

---

## 5. Validation Criteria

- `cargo fmt --all --check` → clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` → zero warnings.
- `cargo test --workspace` → 0 failed (new tests + all M32–M36 tests still green).
- `sdkt package update --check` against a local repo with a satisfied constraint reports
  `up-to-date`; with a newer tag reports `update-available`.
- `rev` pin and `path` dep behavior identical to M36.0 (regression tests).

---

## 6. Completion Criteria

- All deliverables land on `main` via `feat/milestone-37` PR.
- All mandatory quality gates green in CI (fmt / clippy default + all-features / test).
- CHANGELOG `[Unreleased]` carries an "Added — Dependency version resolution (M37)" entry.
- No version bump, tag, publish, or network call beyond the git remote the user declares.

---

## 7. Backward Compatibility

| Area | Impact | Strategy |
|------|--------|----------|
| `Dependency` | ✅ Additive | `version` is `Option<String>` with `#[serde(default)]`; existing manifests parse unchanged. |
| `sdkt.lock` | ✅ Additive | New `version` field optional; old locks still verify. |
| Existing subcommands | ✅ Full | `package update` flags unchanged; `--check`/`--dry-run`/`--format` preserved. |
| `sdkt-core` API | ✅ Additive | Only new `VersionResolver` + field; no changes to existing functions. |
