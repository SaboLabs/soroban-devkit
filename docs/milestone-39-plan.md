# Milestone 39 — Release Polish & SCF Readiness

> **STATUS: SCHEDULED — scope defined, not yet implemented.**
> Promoted to an official roadmap milestone (see `ROADMAP.md` §4 "Package Manager & Distribution").
> No Rust code has been written yet. This document is the implementation contract.

**Branch (when work begins):** `feat/milestone-39`
**Baseline:** M38 merged to `main` (packaging + publish-dry-run available).
**Target:** Harden `sdkt` for **mainnet / production** use and align it with **Stellar Community Fund
(SCF)** grant expectations — without changing the core command surface. This is a polish + readiness
milestone, not a feature milestone.
**Status:** Documented scope — not started.

---

## 1. Objectives

1. **Containerized distribution** — a maintained `Dockerfile` (+ `.dockerignore`) producing a
   reproducible, portable `sdkt` image, satisfying the long-standing RELEASE_READINESS.md "Docker image"
   deferred item and the ROADMAP §6 "Containerized distribution" backlog entry.
2. **Mainnet readiness pass** — ensure every RPC command's network handling is safe for mainnet
   (explicit passphrase, no silent testnet defaults on mutating paths), and that `sdkt verify` /
   `health` / `audit` outputs are suitable for mainnet evidence.
3. **SCF alignment** — produce an `SCF.md` (or extend README) positioning `sdkt` for SCF grant tracks:
   capability summary, architecture fit, license (MIT), and the reproducible-build / offline story that
   grants favor.
4. **Release polish** — refresh `RELEASE_READINESS.md` to the current version, tighten install scripts,
   and add a `sdkt --version` provenance line (commit + build date) gated behind a feature so it stays
   reproducible.

---

## 2. Scope

### In Scope
- `Dockerfile` (multi-stage: build `sdkt` with pinned MSRV `1.88.0`, minimal runtime image) + docs note.
- Mainnet-safety review of RPC command defaults; add explicit guards where a mutating command could hit
  mainnet with a testnet passphrase (reuses `NetworkConfig` / `--network-profile` precedence from M29).
- `docs/scf.md` (or README section) — SCF positioning, capability matrix, reproducible/offline proof.
- `RELEASE_READINESS.md` refreshed to current version + capability list; "Remaining work" updated.
- Optional `--version` provenance (git commit + date) behind a `provenance` feature (default OFF) so
  release binaries stay reproducible; `release.yml` may enable it.
- Docs: README, CHANGELOG (`[Unreleased]`), `docs/cli.md`, `docs/installation.md`, this plan.

### Out of Scope (this milestone)
- New user-facing commands (polish only; no new `sdkt` subcommand beyond what M32–M38 added).
- A hosted package registry (deferred past M38).
- Smart-contract auditing of *third-party* code beyond `sdkt audit` (M13) — only evidence packaging.
- Any crates.io publish of `sdkt` itself in this milestone (that remains `release.yml`'s job).

---

## 3. Architecture

```
Dockerfile                (multi-stage build → minimal runtime image)
   │
docs/scf.md              (grant positioning; no code)
RELEASE_READINESS.md     (doc refresh)
sdkt-cli --version       (provenance feature; reuses env at build time)
sdkt (RPC commands)      (mainnet-safety guards reuse NetworkConfig / M29 precedence)
```

- No new crates. No new core algorithms. Mainnet-safety changes are **guard / default** adjustments on
  top of the existing `NetworkConfig` precedence, not new networking code.
- The `provenance` feature injects `env!("VERGEN_*")` / `git` commit at compile time; it is OFF by default
  so `cargo install --path` and reproducible builds are unaffected.

---

## 4. Deliverables

- `Dockerfile` + `.dockerignore`; documented `docker build` + `docker run sdkt --help` smoke path.
- Mainnet-safety guards on mutating RPC commands (explicit passphrase required when target ≠ default).
- `docs/scf.md` (capability matrix + reproducible/offline evidence for grants).
- `RELEASE_READINESS.md` refreshed (version, capabilities, remaining-work pruned of done items).
- Optional provenance in `sdkt --version` behind `provenance` feature (default OFF).
- Unit/CLI tests: `sdkt --version` shape; mutating command refuses mainnet with testnet default.
- Documentation updates only (plus the Dockerfile, which is not Rust source).

---

## 5. Validation Criteria

- `cargo fmt --all --check` → clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` → zero warnings.
- `cargo test --workspace` → 0 failed (new tests + all M32–M38 tests still green).
- `docker build` succeeds and `docker run --rm sdkt --help` exits 0 (CI job, may be `continue-on-error`).
- `sdkt --version` includes provenance only when `provenance` feature enabled; plain build unchanged.
- Mutating RPC command with a testnet default passphrase targeting mainnet is rejected with a clear error.

---

## 6. Completion Criteria

- All deliverables land on `main` via `feat/milestone-39` PR.
- All mandatory quality gates green in CI; Docker build job green (or explicitly tolerated).
- CHANGELOG `[Unreleased]` carries a "Release polish & SCF readiness (M39)" entry.
- No version bump or tag in this milestone (the actual version bump + tag belongs to the subsequent
  release cut, per RELEASE_READINESS.md §Release process). Network only where the user explicitly
  opts into a mainnet target.

---

## 7. Backward Compatibility

| Area | Impact | Strategy |
|------|--------|----------|
| RPC command defaults | ⚠️ Guard | Mutating commands require explicit passphrase for non-default nets; testnet default retained for testnet. |
| `sdkt --version` | ✅ Additive | Provenance behind opt-in feature; default output unchanged. |
| Existing subcommands | ✅ Full | No command removed or renamed. |
| `sdkt-core` API | ✅ Mostly | Only additive typed guards; no breaking signature changes. |
