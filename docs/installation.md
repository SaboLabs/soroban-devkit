# Installation

`sdkt` is a Rust workspace. You need the Rust stable toolchain (edition 2021)
only if you build from source.

## Recommended — install.sh

Downloads the latest stable release binary, verifies its SHA-256 checksum,
and installs `sdkt` to `~/.local/bin/sdkt` by default. No Rust toolchain
required.

```bash
curl -fsSL https://raw.githubusercontent.com/naninu123/soroban-devkit/main/install.sh | bash
```

The script:
- detects your OS (`Linux` / `macOS`) and architecture (`x86_64` / `aarch64`),
- downloads the matching GitHub Release asset
  (`sdkt-<target>.tar.gz` + `sdkt-<target>.sha256`),
- **verifies the checksum before extracting/running anything**,
- installs the binary and prints `PATH` guidance if needed.

Checksum flow: the script prefers the standalone `sdkt-<target>.sha256`
release asset. If that asset is missing from a release, it falls back to the
embedded `sdkt.sha256` inside the tarball (every release tarball ships it).
Verification is never skipped — if no checksum can be obtained, the install
aborts.

Custom install directory:

```bash
SDKT_INSTALL_DIR=/usr/local/bin curl -fsSL \
  https://raw.githubusercontent.com/naninu123/soroban-devkit/main/install.sh | bash
```

Pin a specific version:

```bash
SDKT_VERSION=v2.4.0 curl -fsSL \
  https://raw.githubusercontent.com/naninu123/soroban-devkit/main/install.sh | bash
```

## Option A — Manual GitHub Release download

1. Open the [Releases](https://github.com/naninu123/soroban-devkit/releases)
   page and download the latest release for your platform:

   | Platform | Asset |
   |----------|-------|
   | Linux (x86_64) | `sdkt-x86_64-unknown-linux-gnu.tar.gz` |
   | macOS (Intel) | `sdkt-x86_64-apple-darwin.tar.gz` |
   | macOS (Apple Silicon) | `sdkt-aarch64-apple-darwin.tar.gz` |

2. Verify the checksum, then extract and run:

   ```bash
   tar -xzf sdkt-x86_64-unknown-linux-gnu.tar.gz   # Linux x86_64
   sha256sum -c sdkt-x86_64-unknown-linux-gnu.sha256
   chmod +x sdkt
   ./sdkt --version
   ```

## Option B — Build from source (recommended for development)

```bash
git clone https://github.com/naninu123/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli
```

This installs the `sdkt` binary into `~/.cargo/bin` (make sure it's on `PATH`).

To install a debug build into the local `target/` dir instead:

```bash
cargo build --bin sdkt
# run via: ./target/debug/sdkt --help
```

## Option C — From crates.io (when published)

```bash
cargo install sdkt-cli
```

> The crate is published on the `v2.4.0` tag and later. If `cargo install
> sdkt-cli` reports "no matching package", build from source (Option B) or
> pin a released tag:
>
> ```bash
> cargo install --git https://github.com/naninu123/soroban-devkit --tag v2.4.0 sdkt-cli --locked
> ```

## Features

`sdkt-cli` has the following optional features:

| Feature | Default | Effect |
|---|---|---|
| `wasm-plugins` | off | Loads cross-platform, sandboxed `.wasm` plugins via `extism` for `sdkt audit`. |
| `plugins` | off | Loads native shared-library plugins (`.so`/`.dylib`/`.dll`) via C-ABI for `sdkt audit`. |
| `provenance` | off | Appends build provenance (git commit + date) to `sdkt --version`. Off by default so release binaries stay reproducible; the release workflow may enable it (`--features provenance`) and supply `SDKT_GIT_COMMIT` / `SDKT_BUILD_DATE` at build time. |

```bash
cargo install --path crates/sdkt-cli --features wasm-plugins
```

## Containerized distribution (M39)

A maintained `Dockerfile` builds a minimal, reproducible `sdkt` image. Build and
smoke-test it locally:

```bash
docker build -t sdkt .
docker run --rm sdkt --help
```

The image runs as a non-root user and only needs network access at runtime when
you point it at a Soroban RPC endpoint. No git metadata or build date is embedded
unless you build with `--features provenance` and supply the provenance env vars.

## Updating

```bash
cd soroban-devkit
git pull
cargo install --path crates/sdkt-cli --force
```

## Verifying your install

```bash
sdkt --version
sdkt --help
```
