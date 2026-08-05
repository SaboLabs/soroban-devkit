# Contributing to Soroban DevKit (`sdkt`)

Thanks for considering a contribution! This document covers how to get set up,
how we review changes, and how releases are produced.

## Development Workflow

1. **Fork and clone** the repository.
2. **Install Rust** (stable toolchain, edition 2021):
   ```bash
   rustup toolchain install stable
   rustup default stable
   ```
3. **Build** to verify your environment:
   ```bash
   cargo build
   ```
4. **Create a branch** for your change (e.g. `feat/foo` or `fix/bar`).
5. **Write tests** for any new functionality or bug fix.
6. **Format & lint** before committing:
   ```bash
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   ```
7. **Test**:
   ```bash
   cargo test --workspace
   ```
## Supply Chain, MSRV & Dependencies

`sdkt` relies on `Cargo.lock` being checked into source control at the workspace root to ensure strictly reproducible builds. 

- **MSRV Policy:** The Minimum Supported Rust Version (MSRV) must not be increased without strict dependency or security justification. If a bump is completely unavoidable, contributors must:
  - Document the required update in `CHANGELOG.md` and `README.md`.
  - Bump `rust-version` in `Cargo.toml`.
  - Update the `msrv` job configuration in `.github/workflows/ci.yml`.
  - Explain precisely why the bump was necessary in the pull request description.
- **Adding Dependencies:** When adding a new library to `Cargo.toml`, ensure that the dependency is genuinely necessary (check if standard library solutions exist first). Use optional dependencies and features extensively to prevent artifact bloat.
- **Pinning & Updating:** Our `Cargo.lock` is pinned manually and updated holistically during specific maintenance phases. Do not run `cargo update` indiscriminately in PRs unrelated to dependency updates.
8. **Open a pull request** against `main` from your branch.

## Testing Instructions

- Unit tests live next to the code (`#[cfg(test)] mod tests`).
- CLI integration tests use `assert_cmd` under `crates/sdkt-cli/tests/`.
  Fixture paths are resolved from `CARGO_MANIFEST_DIR`, so they work regardless
  of the current directory.
- Run the full suite:
  ```bash
  cargo test --workspace
  ```
- Before claiming a CLI change is done, run the **actual binary** against a
  real fixture in both formats:
  ```bash
  cargo run -q --bin sdkt-cli -- diff --old-wasm a.wasm --new-wasm b.wasm --format pretty
  cargo run -q --bin sdkt-cli -- diff --old-wasm a.wasm --new-wasm b.wasm --format json
  ```

## Pull Request Process

- Keep PRs focused on a single change.
- Update `CHANGELOG.md` under the relevant version (or `[Unreleased]`).
- Add tests that verify your change.
- If adding dependencies, update the workspace root `Cargo.toml`
  (`[workspace.package]` / member crates inherit versions).
- Ensure public API signatures are clear and documented (`///` doc comments).
- Fill out the PR template checklist (fmt / clippy / test / docs).

## Code Style

- Use `rustfmt` (`cargo fmt --all`).
- Clippy must pass with `-D warnings`.
- Keep unit tests near the code they test.
- Add doc tests (`///`) for public library functions.

## Types of Contributions

- Bug reports with reproduction steps.
- Feature requests with motivating examples / use cases.
- Pull requests triaged through review.
- Documentation and onboarding improvements.
- Test coverage improvements.

## Good First Issues

Looking for a place to start? Self-contained, low-risk tasks:

- **Docs**: add a copy-paste recipe to `docs/examples.md`, or improve a section
  in `README.md` / `docs/`.
- **CLI polish**: improve `--help` text or error messages in
  `crates/sdkt-cli/src/main.rs` (no behavior change).
- **Tests**: add an integration test under `crates/sdkt-cli/tests/` for an
  existing subcommand.
- **CI**: tighten a workflow without changing release behavior.

Use the **Good First Issue** issue template and label so we can triage.

## Release Process Overview

Releases are cut from `main` on a `v*` tag. The pipeline (see
`.github/workflows/release.yml`):

1. **`validate`** — runs `fmt --check`, `clippy -D warnings`, `test`.
2. **`publish-dry-run`** — `cargo publish --dry-run --workspace` (packages all
   crates locally; does not contact crates.io for unpublished siblings).
3. **`build-binaries`** — cross-compiles release binaries (linux / macos
   intel+arm) and uploads them as artifacts.
4. **`github-release`** — creates the GitHub Release from the tag.
5. **`publish-crates`** — publishes crates in dependency order
   (`sdkt-core → sdkt-xdr → sdkt-wasm → sdkt-rpc → sdkt-storage → sdkt-audit →
   sdkt-audit-example-rule → sdkt-cli`) when `CARGO_REGISTRY_TOKEN` is set.

Versioning follows SemVer from `[workspace.package]` as the single source of
truth. Contributors do **not** bump versions or tag releases — maintainers do
that as part of the release milestone.

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md).
By participating, you agree to uphold it.
