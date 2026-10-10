#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# BOZKURT CODE installer for macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/cumabozkurt/bozkurt-code/main/scripts/install.sh | sh
#
# Installs `bz`, `bozkurtd` and `bozkurt-mcp` from the latest GitHub release into
# $BOZKURT_HOME/bin (default ~/.bozkurt/bin) after verifying their SHA-256.
# Nothing is written outside $BOZKURT_HOME and no root access is needed.
# Linux builds are glibc-based (built on Ubuntu 22.04, glibc >= 2.35); musl/Alpine is not supported.
#
# Environment / flags:
#   BOZKURT_VERSION=v1.0.0   install a specific tag      (--version v1.0.0)
#   BOZKURT_HOME=/path       install root                (--home /path)
#   BOZKURT_REPO=owner/repo  GitHub repository           (--repo owner/repo)
#   BOZKURT_TARGET=triple    override detected target    (--target triple)
#   BOZKURT_BASE_URL=url     download from a mirror instead of GitHub releases
set -eu

REPO="${BOZKURT_REPO:-cumabozkurt/bozkurt-code}"
VERSION="${BOZKURT_VERSION:-latest}"
HOME_DIR="${BOZKURT_HOME:-$HOME/.bozkurt}"
TARGET="${BOZKURT_TARGET:-}"
BINARIES="bz bozkurtd bozkurt-mcp"

say() { printf '%s\n' "bozkurt: $*"; }
err() { printf '%s\n' "bozkurt: error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || err "'$1' is required but was not found"; }

usage() {
  cat <<EOF
Usage: install.sh [--version vX.Y.Z] [--home DIR] [--repo OWNER/REPO] [--target TRIPLE]
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || err "--version needs a value"; VERSION="$2"; shift 2 ;;
    --home)    [ $# -ge 2 ] || err "--home needs a value"; HOME_DIR="$2"; shift 2 ;;
    --repo)    [ $# -ge 2 ] || err "--repo needs a value"; REPO="$2"; shift 2 ;;
    --target)  [ $# -ge 2 ] || err "--target needs a value"; TARGET="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; err "unknown argument: $1" ;;
  esac
done

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *) err "unsupported CPU architecture: $arch" ;;
  esac
  case "$os" in
    Darwin)
      # A shell running under Rosetta reports x86_64; prefer the native build.
      if [ "$arch" = "x86_64" ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then
        arch="aarch64"
      fi
      printf '%s\n' "${arch}-apple-darwin" ;;
    Linux)
      printf '%s\n' "${arch}-unknown-linux-gnu" ;;
    MINGW*|MSYS*|CYGWIN*)
      err "on Windows use scripts/install.ps1 instead" ;;
    *) err "unsupported operating system: $os" ;;
  esac
}

download() { # url dest
  if command -v curl >/dev/null 2>&1; then
    if [ -n "${BOZKURT_BASE_URL:-}" ]; then
      curl -fsSL --retry 3 -o "$2" "$1"
    else
      curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"
    fi
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    err "curl or wget is required"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    err "sha256sum or shasum is required to verify the download"
  fi
}

need uname
need tar
need awk
need mkdir

[ -n "$TARGET" ] || TARGET="$(detect_target)"
ASSET="bozkurt-cli-${TARGET}.tar.gz"

if [ -n "${BOZKURT_BASE_URL:-}" ]; then
  BASE="${BOZKURT_BASE_URL%/}"
elif [ "$VERSION" = "latest" ]; then
  BASE="https://github.com/${REPO}/releases/latest/download"
else
  case "$VERSION" in v*) ;; *) VERSION="v${VERSION}" ;; esac
  BASE="https://github.com/${REPO}/releases/download/${VERSION}"
fi

TMP="$(mktemp -d 2>/dev/null || mktemp -d -t bozkurt)"
trap 'rm -rf "$TMP"' EXIT INT TERM

say "installing BOZKURT CODE ($VERSION) for $TARGET"
say "downloading $BASE/$ASSET"
download "$BASE/$ASSET" "$TMP/$ASSET" || err "download failed (is there a release asset for $TARGET?)"
download "$BASE/SHA256SUMS" "$TMP/SHA256SUMS" || err "could not download SHA256SUMS"

expected="$(awk -v f="$ASSET" '{name=$2; sub(/^\*/, "", name); if (name == f) print $1}' "$TMP/SHA256SUMS")"
[ -n "$expected" ] || err "$ASSET is not listed in SHA256SUMS"
actual="$(sha256_of "$TMP/$ASSET")"
if [ "$expected" != "$actual" ]; then
  err "checksum mismatch for $ASSET (expected $expected, got $actual)"
fi
say "checksum OK ($actual)"

mkdir -p "$TMP/x"
tar -xzf "$TMP/$ASSET" -C "$TMP/x"

BIN_DIR="$HOME_DIR/bin"
mkdir -p "$BIN_DIR"
for b in $BINARIES; do
  src="$(find "$TMP/x" -type f -name "$b" | head -n 1)"
  [ -n "$src" ] || err "archive does not contain '$b'"
  # Write next to the destination, then rename: atomic, and safe while the
  # daemon is running from the old binary.
  cp "$src" "$BIN_DIR/.$b.new"
  chmod 0755 "$BIN_DIR/.$b.new"
  mv -f "$BIN_DIR/.$b.new" "$BIN_DIR/$b"
done

say "installed: $BINARIES -> $BIN_DIR"
if "$BIN_DIR/bz" --version >/dev/null 2>&1; then
  say "$("$BIN_DIR/bz" --version)"
fi

case ":${PATH}:" in
  *":$BIN_DIR:"*) ;;
  *)
    shell_name="$(basename "${SHELL:-sh}")"
    case "$shell_name" in
      zsh)  rc="\$HOME/.zshrc";  line="export PATH=\"$BIN_DIR:\$PATH\"" ;;
      bash) rc="\$HOME/.bashrc (or \$HOME/.bash_profile on macOS)"; line="export PATH=\"$BIN_DIR:\$PATH\"" ;;
      fish) rc="\$HOME/.config/fish/config.fish"; line="fish_add_path $BIN_DIR" ;;
      *)    rc="your shell profile"; line="export PATH=\"$BIN_DIR:\$PATH\"" ;;
    esac
    say ""
    say "$BIN_DIR is not on your PATH. Add this line to $rc:"
    say ""
    say "    $line"
    say ""
    ;;
esac

say "next steps:  bz doctor   then   bz dns setup && bz trust   then   cd your-project && bz link"
