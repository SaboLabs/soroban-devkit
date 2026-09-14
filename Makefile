.PHONY: help build test test-all fmt clippy check ci compat audit-example plugin-pack plugin-verify clean install-dev audit-example-all

## Default: show this help.
help:
	@grep -E '^## ' $(MAKEFILE_LIST) | sed 's/## //' | column -t -s:

## Build all crates (debug).
build:
	cargo build --workspace

## Build the sdkt binary (release).
build-release:
	cargo build --release --bin sdkt

## Run unit + integration tests (default features).
test:
	cargo test --workspace

## Run tests including all features (plugins, wasm-plugins, provenance).
test-all:
	cargo test --workspace --all-features

## Format all code.
fmt:
	cargo fmt --all

## Format + check.
fmt-check:
	cargo fmt --all --check

## Lint with clippy (warnings as errors).
clippy:
	cargo clippy --workspace --all-targets -- -D warnings

## Lint with all features.
clippy-all:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

## Full CI gate locally: fmt + clippy + test.
ci: fmt-check clippy test

## Build the example plugin rule (native .so).
audit-example:
	cargo build -p sdkt-audit-example-rule --features plugins

## Build the example plugin rule (WASM .wasm).
audit-example-wasm:
	cargo build -p sdkt-audit-example-rule --target wasm32-wasip1 --features wasm-plugins

## Build + pack a plugin bundle from the example rule.
plugin-pack:
	@tmpdir=$$(mktemp -d); \
	mkdir -p "$$tmpdir/example"; \
	cp target/debug/libsdkt_audit_example_rule.so "$$tmpdir/example/example.so"; \
	printf 'id = "example-rule"\nname = "Example Rule"\nversion = "1.0.0"\nauthor = "SaboLabs"\ndescription = "Reference audit rule."\nkind = "native"\nartifact = "example.so"\nabi_major = 1\nabi_minor = 0\n' > "$$tmpdir/example/plugin.toml"; \
	./target/debug/sdkt plugin pack "$$tmpdir/example" --output example-rule-1.0.0.sdktplugin; \
	echo "→ bundled example-rule-1.0.0.sdktplugin"

## Verify a plugin bundle.
plugin-verify:
	./target/debug/sdkt plugin verify-bundle example-rule-1.0.0.sdktplugin

## Run the compatibility CI matrix locally.
compat:
	# Mirrors .github/workflows/compatibility.yml without live RPC.
	cargo build --bin sdkt
	cargo build -p sdkt-audit-example-rule --features plugins
	@tmpdir=$$(mktemp -d); \
	mkdir -p "$$tmpdir/example"; \
	cp target/debug/libsdkt_audit_example_rule.so "$$tmpdir/example/example.so"; \
	printf 'id = "example-rule"\nname = "Example Rule"\nversion = "1.0.0"\nauthor = "SaboLabs"\ndescription = "Reference audit rule."\nkind = "native"\nartifact = "example.so"\nabi_major = 1\nabi_minor = 0\n' > "$$tmpdir/example/plugin.toml"; \
	./target/debug/sdkt plugin pack "$$tmpdir/example" --output "$$tmpdir/bundle.sdktplugin"; \
	./target/debug/sdkt plugin verify-bundle "$$tmpdir/bundle.sdktplugin"; \
	echo "compat OK"

## Check formatting, clippy, and tests.
check: fmt-check clippy test

## Remove target dir.
clean:
	cargo clean

## Install the CLI locally (~/bin or cargo bin).
install-dev:
	cargo install --path crates/sdkt-cli --locked
