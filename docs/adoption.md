# Adoption and Integration Evidence

This page lists verifiable examples of where `sdkt` is used in real Soroban
development workflows. Every entry includes a public evidence link and
reproduction instructions.

## Maintained Examples

These examples are owned and maintained by the `soroban-devkit` repository.

### Real-World Contract Compatibility Validation

**Repository:** [stellar/soroban-examples](https://github.com/stellar/soroban-examples)
**sdkt version:** 2.5.0 (workspace version)
**Workflow:** `.github/workflows/compatibility.yml`

The CI pipeline clones the official Stellar example contracts (token,
atomic_swap, liquidity_pool, timelock, single_offer), builds them to WASM,
and runs `sdkt` offline commands against the compiled artifacts:

| Command | Contracts Tested | Result |
|---------|-----------------|--------|
| `sdkt wasm inspect <file>` | 5 | PASS |
| `sdkt diff --old-wasm X --new-wasm X --upgrade-safety` | 5 (self + cross-contract) | PASS |
| `sdkt audit <src.rs>` | 5 | PASS |

**Evidence:**
- Workflow definition: [.github/workflows/compatibility.yml](../.github/workflows/compatibility.yml)
- Compatibility matrix: [docs/compatibility.md](compatibility.md)
- CI runs on every push to `main`/`feat/*` and all PRs:
  [Actions tab](https://github.com/naninu123/soroban-devkit/actions/workflows/compatibility.yml)

**Reproduce from a clean checkout:**

```bash
# 1. Clone sdkt
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo build --bin sdkt
SDKT=$(pwd)/target/debug/sdkt

# 2. Clone example contracts (read-only, shallow)
git clone --depth 1 https://github.com/stellar/soroban-examples /tmp/soroban-examples

# 3. Build contracts to WASM
cd /tmp/soroban-examples
for c in token atomic_swap liquidity_pool timelock single_offer; do
  (cd "$c" && cargo build --target wasm32v1-none --release)
done

# 4. Run sdkt against real artifacts
TOKEN=/tmp/soroban-examples/token/target/wasm32v1-none/release/soroban_token_contract.wasm
$SDKT wasm inspect "$TOKEN"
$SDKT diff --old-wasm "$TOKEN" --new-wasm "$TOKEN" --upgrade-safety
$SDKT audit /tmp/soroban-examples/token/src/lib.rs
```

Requires: Rust stable with `wasm32v1-none` target (`rustup target add wasm32v1-none`).

### CI Test Suite

**Repository:** [naninu123/soroban-devkit](https://github.com/naninu123/soroban-devkit)
**sdkt version:** 2.5.0
**Workflow:** `.github/workflows/ci.yml`

The CI pipeline runs `sdkt` commands as part of the test and validation
infrastructure:

| Command | Purpose | Platforms |
|---------|---------|-----------|
| `cargo test --workspace` | Unit + integration tests (28 test files) | Linux, macOS, Windows |
| `cargo clippy --workspace --all-targets -- -D warnings` | Lint | Ubuntu |
| `cargo fmt --all --check` | Format check | Ubuntu |
| `cargo check --workspace` (Rust 1.88.0) | MSRV validation | Ubuntu |
| `bash install.sh --selftest` | Installer checksum verification | Ubuntu |

**Evidence:**
- Workflow definition: [.github/workflows/ci.yml](../.github/workflows/ci.yml)
- CI runs on every push to `main`/`feat/*` and all PRs:
  [Actions tab](https://github.com/naninu123/soroban-devkit/actions/workflows/ci.yml)

### On-Chain Fixture Validation

**Repository:** [naninu123/soroban-devkit](https://github.com/naninu123/soroban-devkit)
**sdkt version:** 2.5.0
**Workflow:** `.github/workflows/compatibility.yml` (steps M41–M44)

Committed JSON fixtures in `tests/fixtures/onchain/` capture the exact output
of `sdkt` commands against a known testnet contract. CI validates these fixtures
on every run, ensuring the command output schema does not regress:

| Fixture | Command Covered |
|---------|----------------|
| `sample-inspection.json` | `sdkt wasm metadata --contract <id>` |
| `upgrade-verdict.json` | `sdkt verify --contract <id> --wasm <file> --upgrade-safety` |
| `events-abi.json` | `sdkt events <id> --abi-contract <id>` |
| `storage-abi.json` | `sdkt storage analyze <id> --abi-contract <id>` |

**Evidence:**
- Fixture directory: [tests/fixtures/onchain/](../tests/fixtures/onchain/)
- Workflow step definitions: [.github/workflows/compatibility.yml](../.github/workflows/compatibility.yml) (lines 97–236)

## Confirmed External Adopters

No external adopters confirmed yet. See [How to Add Your Integration](#how-to-add-your-integration) below.

## How to Add Your Integration

If you use `sdkt` in your project and would like to be listed here, open a
pull request that adds an entry to this file. Your entry **must** include:

1. **Public repository or contract link** — a URL anyone can visit.
2. **Exact sdkt command or workflow** — the specific `sdkt` invocation you use.
3. **Version tested** — the `sdkt --version` output or Cargo.toml version.
4. **Reproducible evidence** — one of:
   - A link to a CI run (GitHub Actions, etc.) that exercises the command.
   - A script in your repository that reproduces the usage.
   - A PR or issue that documents the integration.
5. **Maintainer attribution** — your name or handle (only if you volunteer it).

### Entry template

Copy this into your PR:

```markdown
### <Project Name>

**Repository:** <URL>
**sdkt version:** <version>
**Workflow:** <description of how sdkt is used>

<What you use sdkt for and which commands.>

**Evidence:**
- <Link to CI run, script, or PR>
```

### Review criteria

Pull requests adding external adopters will be checked for:

- The evidence link is publicly accessible.
- The linked CI run or script actually invokes `sdkt`.
- The version cited matches the evidence (no stale claims).
- No marketing language beyond what the evidence supports.

Entries without verifiable evidence will not be merged.
