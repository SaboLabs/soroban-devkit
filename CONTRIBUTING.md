# Contributing to Soroban DevKit

Welcome! Thanks for considering a contribution. This document explains how to get started.

## Development Workflow

1. **Fork and clone** the repository
2. **Install Rust** (stable toolchain, edition 2021)
3. **Build** with `cargo build` to verify your environment
4. **Create a branch** for your change
5. **Write tests** for any new functionality or bug fix
6. **Format** with `cargo fmt` and lint with `cargo clippy --workspace -- -D warnings`
7. **Test** with `cargo test --workspace`
8. **Open a pull request**

## Pull Request Process

- Keep PRs focused on a single change
- Update CHANGELOG.md under `[Unreleased]` section
- Add tests that verify your change
- If adding dependencies, update workspace's root Cargo.toml in the workspace.dependencies table
- Ensure documentation comments exist where relevant and public API signatures are clear

## Code Style

- Use rustfmt (`cargo fmt`)
- Ensure clippy passes with `-D warnings`
- Keep unit tests near the code they test
- Add doc tests (`///` comments) for public library functions

## Types of Contributions

- Bug reports with reproduction steps
- Feature requests with motivating examples/use cases
- Pull requests triaged through review
- Documentation improvements
- Test coverage improvements
