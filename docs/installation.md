# Installation

`sdkt` is a Rust workspace. You need the Rust stable toolchain (edition 2021).

## Prerequisites

```bash
rustup toolchain install stable
rustup default stable
cargo --version   # >= 1.88.0 required
```

## Option A — Build from source (recommended)

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

## Option B — From crates.io (when published)

```bash
cargo install sdkt
```

> The crate is published on the `v2.0.0` tag and later. If `cargo install
> sdkt` reports "no matching package", build from source (Option A) or
> pin a released tag:
>
> ```bash
> cargo install --git https://github.com/naninu123/soroban-devkit --tag v2.0.0 sdkt-cli --locked
> ```

## Features

`sdkt-cli` has the following optional features:

| Feature | Default | Effect |
|---|---|---|
| `wasm-plugins` | off | Loads cross-platform, sandboxed `.wasm` plugins via `extism` for `sdkt audit`. |
| `plugins` | off | Loads native shared-library plugins (`.so`/`.dylib`/`.dll`) via C-ABI for `sdkt audit`. |

```bash
cargo install --path crates/sdkt-cli --features wasm-plugins
```

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
