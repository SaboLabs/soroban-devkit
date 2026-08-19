# Support

Need help with `sdkt`? This guide points you to the right place.

## Where to ask

| Topic | Where |
|-------|-------|
| Bug reports, crashes, wrong output | [GitHub Issues](https://github.com/SaboLabs/soroban-devkit/issues) |
| General questions, usage help, "how do I…" | [GitHub Discussions](https://github.com/SaboLabs/soroban-devkit/discussions) |
| Security vulnerabilities | [SECURITY.md](SECURITY.md) — Private Security Advisory |
| Contributing code/docs | [CONTRIBUTING.md](CONTRIBUTING.md) |

> If GitHub Discussions is not yet enabled for this repository, please use
> GitHub Issues and label it `question`.

## Before opening an issue

To help us resolve your problem quickly, please:

1. **Check the documentation first** — [README.md](README.md),
   [docs/quick-start.md](docs/quick-start.md), [docs/cli.md](docs/cli.md),
   and [docs/examples.md](docs/examples.md) cover install, commands, and
   common workflows.
2. **Provide your `sdkt` version** — run `sdkt --version` and paste the output.
3. **Provide your environment** — OS and architecture (e.g. `Linux x86_64`,
   `macOS arm64`), and how you installed `sdkt` (release binary, `install.sh`,
   or `cargo install`).
4. **Include reproduction steps** — the exact command(s) you ran, the full
   output (including any error message), and the expected vs. actual behavior.
   For offline commands, attach or reference the WASM/contract input if relevant.

A good bug report looks like:

```
sdkt --version
# sdkt 2.5.0

OS: macOS arm64 (Apple Silicon)
Installed via: install.sh

Command:
  sdkt wasm inspect path/to/contract.wasm

Expected: ABI + storage summary
Actual: error: "..." (full stderr below)
```

## Scope of support

- Offline commands (`decode`, `diff`, `audit`, `build`, `tx sign`, `wasm
  inspect`) need no network and are fully supported locally.
- RPC commands (`inspect`, `storage`, `tx`, `events`, `account`, `fee`, `wasm
  metadata`, `deploy`) require a reachable Soroban RPC endpoint; connectivity
  issues are outside `sdkt`'s control — include the endpoint and any transport
  error you see.

## Contributing

Want to fix something or add a feature? See
[CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow, coding
standards, and pull-request process.
