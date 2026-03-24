#!/usr/bin/env sh
# shp2geojson installer — https://github.com/s19835/shp2geojson
# Usage: curl -fsSL https://raw.githubusercontent.com/s19835/shp2geojson/master/install.sh | sh
set -e

REPO="s19835/shp2geojson"
BIN="shp2geojson"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

# ── Detect OS and arch ────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
      *)       echo "error: unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64)   TARGET="aarch64-apple-darwin" ;;
      x86_64)  TARGET="aarch64-apple-darwin" ;; # Rosetta 2 handles arm64 binary
      *)       echo "error: unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "error: unsupported OS: $OS" >&2
    echo "       Windows users: run install.ps1 in PowerShell" >&2
    exit 1
    ;;
esac

# ── Fetch latest release version ─────────────────────────────────────────────
if command -v curl >/dev/null 2>&1; then
  FETCH="curl -fsSL"
elif command -v wget >/dev/null 2>&1; then
  FETCH="wget -qO-"
else
  echo "error: curl or wget is required" >&2; exit 1
fi

VERSION=$($FETCH "https://api.github.com/repos/$REPO/releases/latest" \
  | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"v\([^"]*\)".*/\1/')

if [ -z "$VERSION" ]; then
  echo "error: could not determine latest version" >&2; exit 1
fi

# ── Download and install ──────────────────────────────────────────────────────
URL="https://github.com/$REPO/releases/download/v${VERSION}/${BIN}-${TARGET}.tar.gz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Installing shp2geojson v${VERSION} for ${TARGET}..."
$FETCH "$URL" | tar -xz -C "$TMP"

mkdir -p "$INSTALL_DIR"
mv "$TMP/$BIN" "$INSTALL_DIR/$BIN"
chmod +x "$INSTALL_DIR/$BIN"

# ── PATH hint ─────────────────────────────────────────────────────────────────
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;  # already in PATH
  *)
    echo ""
    echo "  shp2geojson was installed to $INSTALL_DIR"
    echo "  Add it to your PATH by running:"
    echo ""
    echo "    echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.bashrc && source ~/.bashrc"
    echo ""
    echo "  Or for zsh:"
    echo ""
    echo "    echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc && source ~/.zshrc"
    echo ""
    ;;
esac

echo "Done! Run: shp2geojson --help"
