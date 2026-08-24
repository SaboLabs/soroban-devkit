# Milestone 38 — Packaging & Publishing Workflow

> **STATUS: SCHEDULED — scope defined, not yet implemented.**
> Promoted to an official roadmap milestone (see `ROADMAP.md` §4 "Package Manager & Distribution").
> No Rust code has been written yet. This document is the implementation contract.

**Branch (when work begins):** `feat/milestone-38`
**Baseline:** M37 merged to `main` (dependency version resolution available).
**Target:** A reproducible **packaging** workflow that turns a resolved `.sdkt.toml` + `sdkt.lock`
into a distributable artifact, plus a `publish`-preparation path that validates readiness
(no live registry publish in this milestone unless the operator explicitly enables a registry source).
**Status:** Documented scope — not started.

---

## 1. Objectives

1. Add `sdkt package pack` — produce a self-contained package bundle (tarball / directory) containing
   the manifest, the lockfile, and the resolved dependency checkouts from `.sdkt-cache`, so a project
   can be moved or built on another machine **offline** (reusing the cached git checkouts M35/M36 wrote).
2. Add `sdkt package publish --dry-run` (and a real `--broadcast` only when a registry source exists) —
   validates that the package is publish-ready: manifest valid, lock consistent, all cached commits
   present, integrity hashes match. Reuses `verify_dependencies` and `compute_dependency_integrity`.
3. Emit a machine-readable `package.json`-style manifest (or extend `sdkt.lock`) describing the
   resolved bundle so downstream consumers / CI can verify it.

---

## 2. Scope

### In Scope
- `sdkt package pack [--out <dir>]` — bundles `.sdkt.toml`, `sdkt.lock`, and the `.sdkt-cache/git/<key>`
  checkouts into `<out>/<name>-<version>.tar.zst` (or a directory tree). Reuses the cache layout from
  `GitFetcher` / `git_cache_key`.
- `sdkt package publish --dry-run` — runs the full readiness check (manifest + lock + cache + integrity)
  and prints a publish plan; with an explicit `--broadcast` flag (only honored if a registry source is
  configured) it would perform the publish. **Default is dry-run; no network unless `--broadcast`.**
- A `PackageBundle` descriptor type in `sdkt-core::package` recording the bundle's contents + sha256.
- Unit tests: pack round-trip (pack → unpack → verify identical lock + integrity), dry-run publish plan
  against a local cached project.
- CLI integration tests (offline): `pack` produces an artifact; `publish --dry-run` reports ready.
- Docs: README, CHANGELOG (`[Unreleased]`), `docs/cli.md`, this plan.

### Out of Scope (this milestone)
- Standing up an actual package **registry server / index** (deferred; `DependencyFetcher` abstracts it).
- `cargo publish` of `sdkt` crates — that is release engineering (M39 / existing `release.yml`).
- Signing / notarization of bundles (future work).
- Automatic version bumping on publish (left to the operator / CI).

---

## 3. Architecture

```
sdkt-cli (package pack | package publish)
   │  routes + formats (reuses OutputFormat)
   ▼
sdkt-core::package  (PackageBundle — NEW, pure descriptor + pack/dry-run orchestration)
   │  reuses verify_dependencies, compute_dependency_integrity, git_cache_key
   ▼
sdkt-core::sync / fetch  (cache read via GitFetcher cache layout — no new git logic)
sdkt-core::lock          (read_lock / write_lock / DependencyLock — unchanged)
```

- Packing is a **copy/archive** operation over the existing `.sdkt-cache` checkout tree; it introduces
  no new caching, hashing, or git-clone logic — `compute_dependency_integrity` already hashes the tree.
- Publishing readiness is a **read-only validation** built entirely on `verify_dependencies` +
  `compute_dependency_integrity` (M35.2). No new verification algorithm.

---

## 4. Deliverables

- `sdkt package pack [--out <dir>] [--format tar.zst|dir]` CLI command.
- `sdkt package publish --dry-run [--broadcast]` CLI command (default dry-run).
- `PackageBundle` type + `pack()` / `publish_plan()` functions in `sdkt-core::package`.
- Bundle descriptor emitted alongside the artifact (name, version, lock sha256, per-dep integrity).
- Unit + CLI integration tests (offline; local cache only).
- Documentation updates only.

---

## 5. Validation Criteria

- `cargo fmt --all --check` → clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` → zero warnings.
- `cargo test --workspace` → 0 failed (new tests + all M32–M37 tests still green).
- `sdkt package pack` produces an artifact whose unpacked tree reproduces the original `sdkt.lock`
  and per-dependency integrity hashes byte-for-byte.
- `sdkt package publish --dry-run` exits 0 on a consistent project and non-zero (clear error) when the
  lock drifts from the cache / manifest.

---

## 6. Completion Criteria

- All deliverables land on `main` via `feat/milestone-38` PR.
- All mandatory quality gates green in CI.
- CHANGELOG `[Unreleased]` carries a "Packaging & publishing workflow (M38)" entry.
- No version bump, tag, or crates.io publish. Network only on explicit `--broadcast` with a registry
  source configured (otherwise the command stays offline / dry-run).

---

## 7. Backward Compatibility

| Area | Impact | Strategy |
|------|--------|----------|
| `sdkt.lock` | ✅ Read-only use | Pack/publish read it; no schema change required. |
| `.sdkt-cache` | ✅ Read-only use | Pack copies checkouts; never mutates the cache. |
| Existing subcommands | ✅ Additive | `pack` / `publish` are new `PackageCommand` variants. |
| `sdkt-core` API | ✅ Additive | Only new `PackageBundle` + functions; no changes to existing. |
