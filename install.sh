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
#   bash install.sh --selftest   # offline regression test (no network)
#
# Security:
#   - set -euo pipefail (strict mode)
#   - the downloaded binary is NEVER executed before its checksum is verified
#   - the checksum is verified, preferring the standalone release .sha256
#     asset and falling back to the embedded sdkt.sha256 inside the tarball

set -euo pipefail

REPO="naninu123/soroban-devkit"
INSTALL_DIR="${SDKT_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="sdkt"

err() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*"; }

# ---------------------------------------------------------------------------
# Checksum helpers (kept pure so they can be unit-tested offline)
# ---------------------------------------------------------------------------

# resolve_checksum_to <tarball> <standalone_sha256_path> <out_sha256_path>
# Writes a sdkt.sha256 file to <out_sha256_path>.
# Prefers the standalone asset at <standalone_sha256_path> if it exists,
# otherwise extracts the embedded sdkt.sha256 from <tarball>.
# Returns non-zero if neither is available.
resolve_checksum_to() {
  local tb="$1" standalone="$2" out="$3"
  if [[ -f "$standalone" ]]; then
    cp "$standalone" "$out"
    return 0
  fi
  # Fallback: extract the embedded sdkt.sha256 from the tarball (stdout).
  tar -xzf "$tb" sdkt.sha256 -O > "$out" 2>/dev/null \
    && [[ -s "$out" ]] \
    && return 0
  return 1
}

# verify_binary <tarball> <sha256_path>
# Extracts sdkt from <tarball> and checks it against <sha256_path>.
verify_binary() {
  local tb="$1" sf="$2" d
  d="$(dirname "$tb")"
  ( cd "$d"
    tar -xzf "$(basename "$tb")" sdkt
    $SHASUM -c "$(basename "$sf")" ) || return 1
}

# ---------------------------------------------------------------------------
# Offline self-test (no network, no real install)
# ---------------------------------------------------------------------------
selftest() {
  local d; d="$(mktemp -d)"; trap 'rm -rf "$d"' EXIT
  echo "fake-sdkt-binary" > "$d/sdkt"
  ( cd "$d" && $SHASUM sdkt > sdkt.sha256 )
  tar -czf "$d/sdkt-x86_64-unknown-linux-gnu.tar.gz" -C "$d" sdkt sdkt.sha256

  # Case A: standalone checksum asset exists.
  cp "$d/sdkt.sha256" "$d/sdkt-x86_64-unknown-linux-gnu.sha256"
  resolve_checksum_to "$d/sdkt-x86_64-unknown-linux-gnu.tar.gz" \
    "$d/sdkt-x86_64-unknown-linux-gnu.sha256" "$d/outA.sha256"
  if verify_binary "$d/sdkt-x86_64-unknown-linux-gnu.tar.gz" "$d/outA.sha256"; then
    info "SELFTEST A (standalone checksum) PASS"
  else
    err "SELFTEST A (standalone checksum) FAILED"
  fi

  # Case B: standalone missing -> embedded checksum fallback.
  rm -f "$d/sdkt-x86_64-unknown-linux-gnu.sha256"
  resolve_checksum_to "$d/sdkt-x86_64-unknown-linux-gnu.tar.gz" \
    "$d/does-not-exist.sha256" "$d/outB.sha256" \
    || err "SELFTEST B (embedded fallback) could not resolve checksum"
  if verify_binary "$d/sdkt-x86_64-unknown-linux-gnu.tar.gz" "$d/outB.sha256"; then
    info "SELFTEST B (embedded checksum fallback) PASS"
  else
    err "SELFTEST B (embedded checksum fallback) FAILED"
  fi

  info "ALL SELFTESTS PASS"
  exit 0
}

if [[ "${1:-}" == "--selftest" ]]; then
  # Pick a checksum tool for the offline test before any network logic.
  if command -v sha256sum >/dev/null 2>&1; then
    SHASUM="sha256sum"
  elif command -v shasum >/dev/null 2>&1; then
    SHASUM="shasum -a 256"
  else
    err "neither sha256sum nor shasum available for self-test"
  fi
  selftest
fi

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
# 3. Download tarball + resolve checksum (no execution yet)
# ---------------------------------------------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

info "downloading $TARBALL ..."
curl -fsSL "$BASE/$TARBALL" -o "$TMP/$TARBALL" || err "failed to download $TARBALL"

# Prefer the standalone release checksum asset; fall back to the embedded
# sdkt.sha256 inside the tarball when the standalone asset is absent (e.g.
# some releases only ship the archive). Verification is never skipped.
if curl -fsSL "$BASE/$CHECKSUM" -o "$TMP/$CHECKSUM" 2>/dev/null; then
  info "using standalone checksum asset: $CHECKSUM"
  resolve_checksum_to "$TMP/$TARBALL" "$TMP/$CHECKSUM" "$TMP/sdkt.sha256"
else
  info "standalone $CHECKSUM not found; using embedded sdkt.sha256 from tarball"
  resolve_checksum_to "$TMP/$TARBALL" "$TMP/missing.sha256" "$TMP/sdkt.sha256" \
    || err "no $CHECKSUM asset and tarball has no embedded sdkt.sha256 — cannot verify"
fi

# ---------------------------------------------------------------------------
# 4. Verify checksum BEFORE touching the installed binary
# ---------------------------------------------------------------------------
info "verifying checksum ($SHASUM)..."
verify_binary "$TMP/$TARBALL" "$TMP/sdkt.sha256" \
  || err "checksum verification FAILED — aborting install."

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
