#!/usr/bin/env sh
set -eu

# Usage: curl -sSfL https://<bucket>.s3.amazonaws.com/releases/latest/install.sh | sh
#
# Optional env vars:
#   INSTALL_DIR   where to place the binary (default: ~/.local/bin, fallback: /usr/local/bin)
#   S3_BASE_URL   override the base URL (e.g. for a custom bucket or CloudFront domain)

BINARY="brain3"
S3_BASE_URL="${S3_BASE_URL:-https://__BUCKET__.s3.amazonaws.com/releases/latest}"

# Detect OS
OS="$(uname -s)"
case "$OS" in
  Darwin) OS="apple-darwin" ;;
  Linux)  OS="unknown-linux-gnu" ;;
  *)
    echo "Unsupported OS: $OS" >&2
    exit 1
    ;;
esac

# Detect arch
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64)         ARCH="x86_64" ;;
  aarch64|arm64)  ARCH="aarch64" ;;
  *)
    echo "Unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac

TARGET="${ARCH}-${OS}"
TARBALL="${BINARY}-${TARGET}.tar.gz"
URL="${S3_BASE_URL}/${TARBALL}"

# Resolve install dir
if [ -n "${INSTALL_DIR:-}" ]; then
  BIN_DIR="$INSTALL_DIR"
elif [ -w "/usr/local/bin" ]; then
  BIN_DIR="/usr/local/bin"
else
  BIN_DIR="$HOME/.local/bin"
fi
mkdir -p "$BIN_DIR"

echo "Downloading $BINARY for $TARGET..."
echo "  from: $URL"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

if command -v curl >/dev/null 2>&1; then
  curl -sSfL "$URL" -o "$TMPDIR/$TARBALL"
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$TMPDIR/$TARBALL" "$URL"
else
  echo "Error: neither curl nor wget found" >&2
  exit 1
fi

tar -xzf "$TMPDIR/$TARBALL" -C "$TMPDIR"
chmod +x "$TMPDIR/$BINARY"
mv "$TMPDIR/$BINARY" "$BIN_DIR/$BINARY"

echo ""
echo "Installed $BINARY to $BIN_DIR/$BINARY"

# Warn if install dir is not on PATH
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo ""
    echo "Warning: $BIN_DIR is not in your PATH."
    echo "Add this to your shell profile:"
    echo "  export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac

echo ""
"$BIN_DIR/$BINARY" --version 2>/dev/null && echo "Installation successful." || true
