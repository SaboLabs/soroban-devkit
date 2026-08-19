# VALIDATION_2026-08-19.md

Date: 2026-08-19. Workspace: `/home/ubuntu/soroban-devkit`.
HEAD: `6babff88d718ff780fb35ee388af89019e17d455` (ahead 1 of origin/main `3f7d3956`).
**No reset / rebase / squash / discard.** `6babff8` is HEAD.

Do **not** treat GitHub Actions history as this session's result.

## Preserve check

| Item | Class |
|---|---|
| Unpushed `6babff8` is HEAD | PASS |
| origin/main unchanged `3f7d3956` | PASS |
| Sprint doc diffs still in working tree | PASS |
| `docs/internal/wave-appeal/*` still present | PASS |

## Mandatory gates

| Command | Exit | Exact result | Class |
|---|---|---|---|
| `cargo fmt --all -- --check` | 0 | empty stdout (check-mode) | **PASS** |
| `cargo test --workspace --offline` | 0 | **458 passed, 0 failed, 1 ignored** (46 suites). Ignored: `sdkt-audit` doc-test `register_rule`. `--offline` because crates already cached. | **PASS** |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 0 | `Finished dev profile [unoptimized + debuginfo] target(s) in 3.23s`; zero warnings | **PASS** |

## Extra (not required to unlock Phase 3)

| Command | Exit | Result | Class |
|---|---|---|---|
| `bash scripts/smoke_examples.sh` | 0 | `SMOKE PASS` | PASS |
| `cargo test --manifest-path crates/sdkt-playground/Cargo.toml --offline` | 0 | 8 passed | PASS |
| `./target/debug/sdkt --version` | 0 | `sdkt 2.5.0` | PASS |
| Live Pages + playground + wasm assets HTTP | 200 | see Phase 2 | PASS |
| Browser drop-WASM E2E | — | not executed | **NOT RUN** |
| `curl \| bash install.sh` | — | would mutate `~/.local/bin` | **NOT RUN** |
| `cargo install sdkt-cli` from crates.io | — | not executed | **NOT RUN** |
| GitHub Actions this session | — | last origin success is historical run `31856606475` on `3f7d3956` | **NOT RUN** |

## Code modified during validation?

**No crate / test / workflow edits.**

## Gate

Mandatory trio **PASS** → Phase 3 local audits executed. **No external messages sent.**
