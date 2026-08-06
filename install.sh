#!/usr/bin/env bash
#
# install.sh — one-line installer for sdkt (Soroban DevKit CLI)
#
# Downloads the latest stable release binary from GitHub Releases, verifies
# its SHA-256 checksum, and installs it to ~/.local/bin/sdkt by default.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/naninu123/soroban-devkit/main/install.sh | bash
#   ./install.sh                 # or run locally
#   SDKT_INSTALL_DIR=/usr/local/bin ./install.sh   # custom destination
#
# Security:
#   - set -euo pipefail (strict mode)
#   - the downloaded binary is NEVER executed before its checksum is verified
#   - the checksum is verified against the release-published .sha256 asset

set -euo pipefail

REPO="naninu123/soroban-devkit"
INSTALL_DIR="${SDKT_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="sdkt"

err() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*"; }

# ---------------------------------------------------------------------------
# 1. Detect platform (OS + architecture)
# ---------------------------------------------------------------------------
OS="$(uname -s)"
ARCH="$(uname -m)"

TARGET=""
case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
      *) err "unsupported architecture '$ARCH' on Linux. Supported: x86_64, aarch64." ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-apple-darwin" ;;
      arm64)   TARGET="aarch64-apple-darwin" ;;
      *) err "unsupported architecture '$ARCH' on macOS. Supported: x86_64, arm64 (aarch64)." ;;
    esac
    ;;
  *) err "unsupported operating system '$OS'. Supported: Linux, macOS." ;;
esac

TARBALL="sdkt-${TARGET}.tar.gz"
CHECKSUM="sdkt-${TARGET}.sha256"

info "detected platform: $OS / $ARCH  ->  asset $TARBALL"

# ---------------------------------------------------------------------------
# 2. Resolve the latest stable release tag
# ---------------------------------------------------------------------------
command -v curl >/dev/null 2>&1 || err "curl is required but not found in PATH."
command -v tar  >/dev/null 2>&1 || err "tar is required but not found in PATH."
if command -v sha256sum >/dev/null 2>&1; then
  SHASUM="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  SHASUM="shasum -a 256"
else
  err "neither sha256sum nor shasum is available to verify the checksum."
fi

if [[ -n "${SDKT_VERSION:-}" ]]; then
  VERSION="$SDKT_VERSION"
  info "using pinned version: $VERSION"
else
  info "resolving latest stable release..."
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep -m1 '"tag_name"' \
    | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [[ -n "$VERSION" ]] || err "could not determine the latest release tag from GitHub."
  info "latest stable release: $VERSION"
fi

BASE="https://github.com/${REPO}/releases/download/${VERSION}"

# ---------------------------------------------------------------------------
# 3. Download tarball + checksum into a temp dir (no execution yet)
# ---------------------------------------------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

info "downloading $TARBALL ..."
curl -fsSL "$BASE/$TARBALL"  -o "$TMP/$TARBALL"  || err "failed to download $TARBALL"
info "downloading $CHECKSUM ..."
curl -fsSL "$BASE/$CHECKSUM" -o "$TMP/$CHECKSUM" || err "failed to download $CHECKSUM"

# ---------------------------------------------------------------------------
# 4. Verify checksum BEFORE touching the binary
# ---------------------------------------------------------------------------
info "verifying checksum ($SHASUM)..."
# The published .sha256 contains a single line: "<hash>  sdkt"
# Verify it against the binary by checking inside the tarball's expected name.
( cd "$TMP"
  tar -xzf "$TARBALL" sdkt sdkt.sha256
  # Normalize the checksum filename so sha256sum -c can match "sdkt".
  cp "$CHECKSUM" sdkt.sha256
  $SHASUM -c sdkt.sha256 || err "checksum verification FAILED — aborting install."
)
info "checksum OK"

# ---------------------------------------------------------------------------
# 5. Install
# ---------------------------------------------------------------------------
mkdir -p "$INSTALL_DIR"
install -m 0755 "$TMP/sdkt" "$INSTALL_DIR/$BINARY_NAME"
info "installed $BINARY_NAME to $INSTALL_DIR/$BINARY_NAME"

# ---------------------------------------------------------------------------
# 6. PATH guidance
# ---------------------------------------------------------------------------
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo
    echo "NOTE: $INSTALL_DIR is not on your PATH."
    echo "Add it to your shell profile, e.g.:"
    echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> \"\$HOME/.${SHELL##*/}rc\""
    echo "Then restart your shell or run: export PATH=\"\$HOME/.local/bin:\$PATH\""
    ;;
esac

echo
echo "Done. Verify with:"
echo "  sdkt --version"
echo "  sdkt --help"
