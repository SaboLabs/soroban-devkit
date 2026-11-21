#!/usr/bin/env bash
#
# install.sh — one-line installer for sdkt (Soroban DevKit CLI)
#
# Downloads the latest stable release binary from GitHub Releases, verifies
# its SHA-256 checksum, and installs it to ~/.local/bin/sdkt (or
# ~/.local/bin/sdkt.exe on Windows) by default.
#
# Usage:
# curl -fsSL https://raw.githubusercontent.com/SaboLabs/soroban-devkit/main/install.sh | bash
# ./install.sh # or run locally
# SDKT_INSTALL_DIR=/usr/local/bin ./install.sh # custom destination
# bash install.sh --selftest # offline regression test (no network)
#
# Environment variables (both optional):
#   SDKT_VERSION       Release tag to install, exactly as published (e.g. v2.7.0).
#                      Default: the latest stable release.
#   SDKT_INSTALL_DIR   Directory to install sdkt into; created if missing.
#                      Default: $HOME/.local/bin
#
# Security:
# - set -euo pipefail (strict mode)
# - the downloaded binary is NEVER executed before its checksum is verified
# - the checksum is verified, preferring the standalone release .sha256
#   asset and falling back to the embedded sdkt.sha256 inside the archive
#
# Supported platforms:
#   Linux        x86_64, aarch64   (sdkt-<target>.tar.gz)
#   macOS        x86_64, arm64     (sdkt-<target>.tar.gz)
#   Windows      x86_64            (sdkt-<target>.zip, via Git Bash / MSYS /
#                                  Cygwin; extracted with PowerShell)

set -euo pipefail

REPO="SaboLabs/soroban-devkit"
INSTALL_DIR="${SDKT_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="sdkt"

err() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*"; }

# _powershell_bin
# Prints the PowerShell executable to use (pwsh preferred, Windows PowerShell
# 5.1's `powershell` as fallback). Returns non-zero if neither is available.
_powershell_bin() {
  if command -v pwsh >/dev/null 2>&1; then
    printf 'pwsh'
  elif command -v powershell >/dev/null 2>&1; then
    printf 'powershell'
  else
    return 1
  fi
}

# download_asset <asset>
# Downloads <asset> from the release BASE URL into "$TMP/<asset>".
# On failure, the "no asset for this platform" guidance is emitted only when a
# HEAD request confirms HTTP 404. A 503 or other transport/server failure is
# reported as a generic download failure instead of a misleading 404 message.
download_asset() {
  local asset="$1" status="000"
  local url="$BASE/$asset"
  if curl -fsSL "$url" -o "$TMP/$asset"; then
    return 0
  fi

  # First confirm the release tag exists, then inspect the asset URL's HTTP
  # status explicitly. Only a confirmed 404 means the asset is genuinely
  # absent from an otherwise valid release.
  if curl -fsSL -o /dev/null \
    "https://api.github.com/repos/${REPO}/releases/tags/${VERSION}" 2>/dev/null; then
    status="$(curl -sS -o /dev/null -I -L -w '%{http_code}' "$url" 2>/dev/null)" \
      || status="000"
    if [[ "$status" == "404" ]]; then
      err "release ${VERSION} has no ${asset} asset for this platform.
       Build from source instead:
         git clone https://github.com/${REPO}
         cd soroban-devkit && cargo install --path crates/sdkt-cli
       Or install an older/newer release that ships this target:
         SDKT_VERSION=<tag> bash install.sh"
    fi
  fi
  err "failed to download ${asset} (HTTP status: ${status})"
}

# print_path_guidance
# If INSTALL_DIR is not already on PATH, print commands for adding it.
# INSTALL_DIR is shell-escaped with printf %q so the printed commands are safe
# to paste even when the directory contains spaces, quotes or metacharacters.
# Both interactive and login-shell startup files are named because shells read
# different files depending on how they are launched.
print_path_guidance() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return 0 ;;
  esac

  local dir_q shell_name="${SHELL:-/bin/sh}"
  printf -v dir_q '%q' "$INSTALL_DIR"

  echo
  echo "NOTE: $INSTALL_DIR is not on your PATH."
  case "${shell_name##*/}" in
    zsh)
      echo "Add it to ~/.zshrc (interactive shells) or ~/.zprofile (login shells):"
      ;;
    bash)
      echo "Add it to ~/.bashrc (interactive shells) or ~/.bash_profile / ~/.profile (login shells):"
      ;;
    *)
      echo "Add it to your shell startup file (e.g. ~/.profile, or your shell's login equivalent):"
      ;;
  esac
  echo
  # shellcheck disable=SC2016  # $PATH is intentionally literal in the printed guidance
  printf '  export PATH=%s:$PATH\n' "$dir_q"
  echo
  echo "Then restart your shell, or run this now:"
  # shellcheck disable=SC2016  # $PATH is intentionally literal in the printed guidance
  printf '  export PATH=%s:$PATH\n' "$dir_q"
}

# ---------------------------------------------------------------------------
# Checksum helpers (kept pure so they can be unit-tested offline)
# ---------------------------------------------------------------------------

# resolve_checksum_to <archive> <standalone_sha256_path> <out_sha256_path>
# Writes a sdkt.sha256 file to <out_sha256_path>.
# Prefers the standalone asset at <standalone_sha256_path> if it exists,
# otherwise extracts the embedded sdkt.sha256 from <archive>.
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

# verify_binary <archive> <sha256_path>
# Extracts sdkt from <archive> and checks it against <sha256_path>.
verify_binary() {
  local tb="$1" sf="$2" d
  d="$(dirname "$tb")"
  ( cd "$d"
    tar -xzf "$(basename "$tb")" sdkt
    $SHASUM -c "$(basename "$sf")" ) || return 1
}

# ---------------------------------------------------------------------------
# Windows archive helpers (no `tar` guarantee on Windows; PowerShell is used)
# ---------------------------------------------------------------------------

# extract_zip <zipfile> <outdir>
# Extracts a ZIP archive using PowerShell Expand-Archive.
# Paths are converted with cygpath -m and passed as bound parameters to a
# parameterized PowerShell script invoked via -File (never interpolated into
# command text), so paths with spaces or backslashes are handled safely.
# -LiteralPath treats valid paths containing wildcard characters (e.g. [ ])
# literally, and -ErrorAction Stop makes extraction failures return a nonzero
# status. Works with both Windows PowerShell 5.1 and PowerShell 7.
extract_zip() {
  local zip="$1" out="$2" winzip winout ps ps_script win_script
  winzip="$(cygpath -m "$zip" 2>/dev/null)" || winzip="$zip"
  winout="$(cygpath -m "$out" 2>/dev/null)" || winout="$out"
  ps_script="$TMP/sdkt-extract-$$.ps1"
  cat > "$ps_script" <<'EOF'
param([string]$Path, [string]$DestinationPath)
Expand-Archive -LiteralPath $Path -DestinationPath $DestinationPath -Force -ErrorAction Stop
EOF
  win_script="$(cygpath -m "$ps_script" 2>/dev/null)" || win_script="$ps_script"
  ps="$(_powershell_bin)" \
    || err "PowerShell (pwsh or powershell) is required to extract the Windows ZIP but was not found."
  "$ps" -NoProfile -ExecutionPolicy Bypass -File "$win_script" -Path "$winzip" -DestinationPath "$winout" >/dev/null 2>&1 \
    && return 0
  err "failed to extract ZIP archive with PowerShell."
}

# extract_embedded_checksum <zipfile> <out_sha256_path>
# Extracts the embedded sdkt.sha256 entry from a Windows ZIP using PowerShell.
# Paths are converted with cygpath -m and passed as bound parameters to a
# parameterized PowerShell script invoked via -File (never interpolated into
# command text). Returns 0 on success, non-zero if the sdkt.sha256 entry is
# absent or extraction fails. Works with both Windows PowerShell 5.1 and 7.
extract_embedded_checksum() {
  local zip="$1" out="$2" winzip winout ps ps_script win_script result=1
  winzip="$(cygpath -m "$zip" 2>/dev/null)" || winzip="$zip"
  winout="$(cygpath -m "$out" 2>/dev/null)" || winout="$out"
  ps_script="$TMP/sdkt-checksum-$$.ps1"
  cat > "$ps_script" <<'EOF'
param([string]$Path, [string]$DestinationPath)
Add-Type -AssemblyName System.IO.Compression.FileSystem
$result = 1
$z = $null
try {
    $z = [System.IO.Compression.ZipFile]::OpenRead($Path)
    $e = $z.Entries | Where-Object { $_.Name -eq 'sdkt.sha256' } | Select-Object -First 1
    if ($null -ne $e) {
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($e, $DestinationPath, $true)
        $result = 0
    }
}
catch {
    $result = 1
}
finally {
    if ($null -ne $z) { $z.Dispose() }
}
exit $result
EOF
  win_script="$(cygpath -m "$ps_script" 2>/dev/null)" || win_script="$ps_script"
  ps="$(_powershell_bin)" || return 1
  "$ps" -NoProfile -ExecutionPolicy Bypass -File "$win_script" -Path "$winzip" -DestinationPath "$winout" >/dev/null 2>&1 \
    && result=0
  return $result
}

# ---------------------------------------------------------------------------
# Offline self-test helpers (no network, no real install)
# ---------------------------------------------------------------------------

# _selftest_windows_target <os> [arch]
# Mirrors the real platform detection: Git Bash / MSYS / Cygwin and Windows_NT
# uname -s values all normalize to the Windows x86_64 target. Used by the
# offline self-test so the Windows detection logic is exercised on Linux CI
# runners without any network call.
_selftest_windows_target() {
  local os="$1" arch="${2:-x86_64}" target=""
  case "$os" in
    MINGW*|MSYS*|CYGWIN*|Windows_NT) os="Windows" ;;
  esac
  case "$os" in
    Windows)
      case "$arch" in
        x86_64) target="x86_64-pc-windows-msvc" ;;
      esac
      ;;
  esac
  printf '%s' "$target"
}

# _selftest_make_zip <srcdir> <zipfile>
# Builds a ZIP fixture from the contents of <srcdir> using PowerShell's
# Compress-Archive. Git Bash does not ship `zip`, so the Windows extraction
# helpers cannot be exercised without building the fixture this way.
_selftest_make_zip() {
  local src="$1" zipfile="$2" win_src win_zip ps ps_script win_script
  win_src="$(cygpath -m "$src" 2>/dev/null)" || win_src="$src"
  win_zip="$(cygpath -m "$zipfile" 2>/dev/null)" || win_zip="$zipfile"
  ps="$(_powershell_bin)" || return 1
  ps_script="$TMP/sdkt-makezip-$$.ps1"
  cat > "$ps_script" <<'EOF'
param([string]$SourceDir, [string]$DestinationPath)
Compress-Archive -Path (Join-Path $SourceDir '*') -DestinationPath $DestinationPath -Force -ErrorAction Stop
EOF
  win_script="$(cygpath -m "$ps_script" 2>/dev/null)" || win_script="$ps_script"
  "$ps" -NoProfile -ExecutionPolicy Bypass -File "$win_script" \
    -SourceDir "$win_src" -DestinationPath "$win_zip" >/dev/null 2>&1
}

selftest() {
  local d; d="$(mktemp -d)"; TMP="$d"; trap 'rm -rf "$d"' EXIT
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

 # Case C: Windows OS detection (no network, no real install).
  local os
  for os in "MINGW64_NT-10.0-19045" "MSYS_NT-10.0" "CYGWIN_NT-10.0" "Windows_NT"; do
    local r; r="$(_selftest_windows_target "$os")"
    [[ "$r" == "x86_64-pc-windows-msvc" ]] \
      || err "SELFTEST C: expected x86_64-pc-windows-msvc for OS '$os', got '$r'"
  done
  # Non-Windows values must not resolve to a Windows target.
  for os in "Linux" "Darwin" "FreeBSD"; do
    local r; r="$(_selftest_windows_target "$os")"
    [[ -z "$r" ]] \
      || err "SELFTEST C: expected empty for non-Windows OS '$os', got '$r'"
  done
  info "SELFTEST C (Windows OS detection) PASS"

 # Case D: Windows ZIP extraction (only when PowerShell is available; skipped
 # on Linux CI runners). The workspace, ZIP, extraction directory and checksum
 # output paths all contain spaces, exercising both PowerShell helpers with
 # spaces in every path. The real SHA-256 digest of sdkt.exe is verified
 # against both the standalone and embedded checksum paths.
 #
 # NOTE: this exercises only the PowerShell helpers under Git Bash; it does not
 # perform a full installer run against GitHub Releases.
  if _powershell_bin >/dev/null 2>&1; then
    local ws; ws="$TMP/sdkt selftest workspace"
    local src; src="$ws/src"
    mkdir -p "$src"
    echo "fake-sdkt-exe" > "$src/sdkt.exe"
    ( cd "$src" && $SHASUM sdkt.exe > sdkt.sha256 )

    local zip_path; zip_path="$ws/sdkt x86_64-pc-windows-msvc.zip"
    _selftest_make_zip "$src" "$zip_path" \
      || err "SELFTEST D: could not build ZIP fixture"

    # Standalone checksum asset = copy of the checksum shipped in the ZIP.
    local standalone; standalone="$ws/standalone checksum.sha256"
    cp "$src/sdkt.sha256" "$standalone"

    # (1) Extract via PowerShell into a directory whose path has spaces, then
    #     verify the actual digest against the standalone checksum.
    local extract_dir; extract_dir="$ws/extract dir"
    extract_zip "$zip_path" "$extract_dir"
    ( cd "$extract_dir" && $SHASUM -c "$standalone" ) \
      || err "SELFTEST D: standalone checksum verification FAILED"
    info "SELFTEST D (ZIP extraction + standalone digest, paths with spaces) PASS"

    # (2) Extract the embedded checksum via PowerShell into a path with spaces,
    #     then verify the same binary's digest against it.
    local embedded; embedded="$ws/embedded checksum.sha256"
    extract_embedded_checksum "$zip_path" "$embedded" \
      || err "SELFTEST D: embedded sdkt.sha256 not extracted from ZIP"
    ( cd "$extract_dir" && $SHASUM -c "$embedded" ) \
      || err "SELFTEST D: embedded checksum verification FAILED"
    info "SELFTEST D (embedded checksum digest, paths with spaces) PASS"

    # (3) A ZIP without an embedded sdkt.sha256 must return a nonzero status.
    local no_sum_src; no_sum_src="$ws/src without checksum"
    local no_sum_zip; no_sum_zip="$ws/no embedded checksum.zip"
    mkdir -p "$no_sum_src"
    echo "fake-sdkt-exe" > "$no_sum_src/sdkt.exe"
    _selftest_make_zip "$no_sum_src" "$no_sum_zip" \
      || err "SELFTEST D: could not build no-checksum ZIP fixture"
    if extract_embedded_checksum "$no_sum_zip" "$ws/should not exist.sha256"; then
      err "SELFTEST D: missing embedded checksum unexpectedly returned success"
    fi
    info "SELFTEST D (missing embedded checksum returns nonzero) PASS"
  else
    info "SELFTEST D skipped: PowerShell not available on this runner."
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

# Normalize Windows OS values reported by Git Bash / MSYS / Cygwin and by
# Windows_NT (native Windows shells) so the case below can branch on a
# single "Windows" value.
# uname -s on Git Bash:   MINGW64_NT-10.0-19045 (or MINGW32_NT-...)
# uname -s on MSYS:       MSYS_NT-10.0 (or MSYS_NT-6.1, ...)
# uname -s on Cygwin:     CYGWIN_NT-10.0 (or CYGWIN_NT-...)
# uname -s on Windows_NT: Windows_NT
case "$OS" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT) OS="Windows" ;;
esac

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
  Windows)
    case "$ARCH" in
      x86_64) TARGET="x86_64-pc-windows-msvc" ;;
      *) err "unsupported architecture '$ARCH' on Windows. Supported: x86_64." ;;
    esac
    ;;
  *) err "unsupported operating system '$OS'. Supported: Linux, macOS, Windows." ;;
esac

if [[ "$OS" == "Windows" ]]; then
  ASSET="sdkt-${TARGET}.zip"
else
  TARBALL="sdkt-${TARGET}.tar.gz"
  ASSET="$TARBALL"
fi
CHECKSUM="sdkt-${TARGET}.sha256"

info "detected platform: $OS / $ARCH  ->  asset $ASSET"

# ---------------------------------------------------------------------------
# 2. Prerequisites shared by every platform
# ---------------------------------------------------------------------------
command -v curl >/dev/null 2>&1 || err "curl is required but not found in PATH."

# A checksum tool is required on every platform (Git Bash ships sha256sum).
if command -v sha256sum >/dev/null 2>&1; then
  SHASUM="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  SHASUM="shasum -a 256"
else
  err "neither sha256sum nor shasum is available to verify the checksum."
fi

# Platform-specific prerequisite: tar on Unix, PowerShell on Windows.
if [[ "$OS" == "Windows" ]]; then
  if ! command -v pwsh >/dev/null 2>&1 && ! command -v powershell >/dev/null 2>&1; then
    err "PowerShell (pwsh or powershell) is required to install on Windows but was not found."
  fi
else
  command -v tar  >/dev/null 2>&1 || err "tar is required but not found in PATH."
fi

# ---------------------------------------------------------------------------
# 3. Resolve the latest stable release tag
# ---------------------------------------------------------------------------
if [[ -n "${SDKT_VERSION:-}" ]]; then
  VERSION="$SDKT_VERSION"
  info "using pinned version: $VERSION"
else
  info "resolving latest stable release..."
  # Read the whole response before extracting the tag: piping curl into
  # `grep -m1` closes the pipe early, and under `set -o pipefail` curl's
  # SIGPIPE (exit 23) would abort the install on a perfectly good response.
  RELEASE_JSON="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")"
  VERSION="$(printf '%s\n' "$RELEASE_JSON" \
    | grep -m1 '"tag_name"' \
    | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [[ -n "$VERSION" ]] || err "could not determine the latest release tag from GitHub."
  info "latest stable release: $VERSION"
fi

BASE="https://github.com/${REPO}/releases/download/${VERSION}"

# ---------------------------------------------------------------------------
# Temp directory (shared by both platform paths)
# ---------------------------------------------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ---------------------------------------------------------------------------
# Windows install path: download .zip, extract via PowerShell, verify the
# checksum, install sdkt.exe, then exit. Linux/macOS continue below.
# ---------------------------------------------------------------------------
if [[ "$OS" == "Windows" ]]; then
  ZIP="sdkt-${TARGET}.zip"

  info "downloading $ZIP ..."
  download_asset "$ZIP"

  # Resolve checksum: prefer the standalone release checksum asset; fall back
  # to the embedded sdkt.sha256 inside the zip. Verification is never skipped.
  SDKT_SHA256="$TMP/sdkt.sha256"
  if curl -fsSL "$BASE/$CHECKSUM" -o "$TMP/$CHECKSUM" 2>/dev/null; then
    info "using standalone checksum asset: $CHECKSUM"
    cp "$TMP/$CHECKSUM" "$SDKT_SHA256"
  else
    info "standalone $CHECKSUM not found; using embedded sdkt.sha256 from zip"
    extract_embedded_checksum "$TMP/$ZIP" "$SDKT_SHA256" \
      || err "no $CHECKSUM asset and zip has no embedded sdkt.sha256 — cannot verify"
  fi

  # Extract sdkt.exe from the zip into a dedicated directory (no tar
  # guarantee on Windows). Extracting into $TMP itself would overwrite
  # $SDKT_SHA256 ($TMP/sdkt.sha256), which holds the standalone checksum
  # used for verification — the checksum stays outside the extraction dir.
  EXTRACT_DIR="$TMP/extract"
  info "extracting $ZIP ..."
  extract_zip "$TMP/$ZIP" "$EXTRACT_DIR"

  # Verify checksum BEFORE touching the installed binary.
  info "verifying checksum ($SHASUM)..."
  ( cd "$EXTRACT_DIR" && $SHASUM -c "$SDKT_SHA256" ) \
    || err "checksum verification FAILED — aborting install."

  # Install sdkt.exe.
  mkdir -p "$INSTALL_DIR"
  install -m 0755 "$EXTRACT_DIR/sdkt.exe" "$INSTALL_DIR/sdkt.exe"
  info "installed sdkt.exe to $INSTALL_DIR/sdkt.exe"

  # -------------------------------------------------------------------------
  # PATH guidance
  # -------------------------------------------------------------------------
  print_path_guidance

  echo
  echo "Done. Verify with:"
  echo "  sdkt.exe --version"
  echo "  sdkt.exe --help"
  exit 0
fi

# ---------------------------------------------------------------------------
# 4. Download archive + resolve checksum (no execution yet)
# ---------------------------------------------------------------------------
info "downloading $TARBALL ..."
download_asset "$TARBALL"

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
# 5. Verify checksum BEFORE touching the installed binary
# ---------------------------------------------------------------------------
info "verifying checksum ($SHASUM)..."
verify_binary "$TMP/$TARBALL" "$TMP/sdkt.sha256" \
  || err "checksum verification FAILED — aborting install."

# ---------------------------------------------------------------------------
# 6. Install
# ---------------------------------------------------------------------------
mkdir -p "$INSTALL_DIR"
install -m 0755 "$TMP/sdkt" "$INSTALL_DIR/$BINARY_NAME"
info "installed $BINARY_NAME to $INSTALL_DIR/$BINARY_NAME"

# ---------------------------------------------------------------------------
# 7. PATH guidance
# ---------------------------------------------------------------------------
print_path_guidance

echo
echo "Done. Verify with:"
echo "  sdkt --version"
echo "  sdkt --help"
