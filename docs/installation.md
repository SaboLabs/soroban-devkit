# Installation

`sdkt` is a Rust workspace. You need the Rust stable toolchain (edition 2021)
only if you build from source.

## Recommended — install.sh

Downloads the latest stable release binary, verifies its SHA-256 checksum,
and installs `sdkt` to `~/.local/bin/sdkt` by default. No Rust toolchain
required.

```bash
curl -fsSL https://raw.githubusercontent.com/SaboLabs/soroban-devkit/main/install.sh | bash
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
  https://raw.githubusercontent.com/SaboLabs/soroban-devkit/main/install.sh | bash
```

Pin a specific version:

```bash
SDKT_VERSION=v2.5.0 curl -fsSL \
  https://raw.githubusercontent.com/SaboLabs/soroban-devkit/main/install.sh | bash
```

## Option A — Manual GitHub Release download

1. Open the [Releases](https://github.com/SaboLabs/soroban-devkit/releases)
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

## Windows

Windows x86_64 is tested in CI. **v2.5.0 GitHub Release does not include**
`sdkt-x86_64-pc-windows-msvc.zip` (linux + macOS tarballs only). Use crates.io
or source until the next tag, whose `release.yml` builds that zip.

### Option A — Prebuilt binary (next tagged release)

1. Open the [Releases](https://github.com/SaboLabs/soroban-devkit/releases)
   page. If a `sdkt-x86_64-pc-windows-msvc.zip` asset exists for that tag,
   download it. If not, skip to Option B (`cargo install sdkt-cli`).

2. Extract and run (PowerShell):

   ```powershell
   Expand-Archive -Path sdkt-x86_64-pc-windows-msvc.zip -DestinationPath .
   .\sdkt.exe --version
   ```

3. Optional: add the directory containing `sdkt.exe` to your `PATH`:

   ```powershell
   $env:Path += ";C:\path\to\sdkt-directory"
   ```

   To make the change permanent, add it in **System Properties → Environment Variables**.

### Option B — Build from source

Requires Rust 1.88.0+:

```powershell
git clone https://github.com/SaboLabs/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli
sdkt --version
```

This installs `sdkt.exe` into `%USERPROFILE%\.cargo\bin`. Make sure it's on your `PATH`.

## Option B (cross-platform) — Build from source

```bash
git clone https://github.com/SaboLabs/soroban-devkit
cd soroban-devkit
cargo install --path crates/sdkt-cli
```

This installs the `sdkt` binary into `~/.cargo/bin` (make sure it's on `PATH`).

To install a debug build into the local `target/` dir instead:

```bash
cargo build --bin sdkt
# run via: ./target/debug/sdkt --help
```

## Option C — From crates.io (published)

All workspace crates are published to crates.io (current release `v2.5.0`):
`sdkt-cli`, `sdkt-core`, `sdkt-xdr`, `sdkt-wasm`, `sdkt-rpc`, `sdkt-storage`,
`sdkt-audit`, and `sdkt-audit-example-rule`. Install the CLI binary directly:

```bash
cargo install sdkt-cli
sdkt --version
```

To pin a released version:

```bash
cargo install sdkt-cli --version 2.5.0
```

The `sdkt` binary name is reserved by `sdkt-cli` (the published package name is
`sdkt-cli`); `cargo install sdkt-cli` installs the `sdkt` binary. Building from
source (Options A/B) remains supported and is equivalent.

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
