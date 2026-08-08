# syntax=docker/dockerfile:1
#
# Soroban DevKit (`sdkt`) — containerized distribution (M39).
#
# Multi-stage build: a pinned Rust toolchain (MSRV 1.88.0) compiles the CLI,
# then a minimal distroless/runtime image carries only the statically linked
# `sdkt` binary. The build is reproducible: no git metadata or build date is
# embedded unless explicitly supplied via the `provenance` feature.

# ---------------------------------------------------------------------------
# Stage 1 — build
# ---------------------------------------------------------------------------
FROM rust:1.88.0-bookworm AS builder

# Avoid interactive prompts during package installs.
ENV DEBIAN_FRONTEND=noninteractive

WORKDIR /usr/src/sdkt

# Use a separate Cargo home to keep layer caching predictable and avoid pulling
# the host's registry cache into the image.
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:${PATH}

# Copy manifests first for dependency-layer caching.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build the release binary. Crates are offline-capable; the CLI talks to remote
# RPC only at runtime, so the build itself needs no network beyond fetching
# crates (which Cargo does from crates.io).
RUN cargo build --release --bin sdkt

# ---------------------------------------------------------------------------
# Stage 2 — minimal runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# `ca-certificates` so the binary can reach HTTPS Soroban RPC endpoints at runtime.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Run as a non-root user.
RUN useradd --create-home --uid 10001 sdkt

WORKDIR /home/sdkt

COPY --from=builder /usr/src/sdkt/target/release/sdkt /usr/local/bin/sdkt

USER sdkt

ENTRYPOINT ["sdkt"]
CMD ["--help"]
