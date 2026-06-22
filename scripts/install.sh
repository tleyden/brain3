#!/usr/bin/env sh
set -eu

# Usage: curl -sSfL https://<bucket>.s3.amazonaws.com/releases/latest/install.sh | sh
#
# Optional env vars:
#   INSTALL_DIR   where to place the binary (default: ~/.local/bin, fallback: /usr/local/bin)
#   S3_BASE_URL   override the base URL (e.g. for a custom bucket or CloudFront domain)
#   BRAIN3_RELEASE_SIGNING_PUBLIC_KEY_FILE
#                 override the embedded release signing public key (useful for tests)

BINARY="brain3"
CHECKSUMS_FILE="SHA256SUMS"
SIGNATURE_FILE="SHA256SUMS.sig"
S3_BASE_URL="${S3_BASE_URL:-https://__BUCKET__.s3.amazonaws.com/releases/latest}"

fail() {
  echo "Error: $*" >&2
  exit 1
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    fail "required command not found: $1"
  fi
}

download_file() {
  src="$1"
  dst="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -sSfL "$src" -o "$dst"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$dst" "$src"
  else
    fail "neither curl nor wget found"
  fi
}

write_embedded_public_key() {
  cat > "$1" <<'EOF'
-----BEGIN PUBLIC KEY-----
MIICIjANBgkqhkiG9w0BAQEFAAOCAg8AMIICCgKCAgEAt2Cd/7Um2hJ7qDw1BOGv
sobVpu0Vn5YW4JiX+KinfqmFDMXZaNHbLcfJOjKXx8BjoT8hyILuUKBZrcdwQais
FHZ11PeB3pRuumW1cA2lJs0cgz25IMnN+6Y4H1IpmL1V5FpbK9qGvAe76oM2LBsb
/cIrfig31WzhyLHBBRAosK4L9SdMlBFeKYNyJxlTiG8TOo1VXuTgEZRECgg+HhSe
YQuQSQ2jNPl00vF0lA9vqqXRh84Jt+0yTkmRDHE2aft09gSEhdYfxyZHX+SX5tDQ
voGbPr40TeL5uyT3JiOtqRxXRKudjZdvr6g6ojSqzUj+i2n02JCWsEBhMR2j5eZi
UVPCt6B9Mf2PlEZjoYPFIGCY7rd8bAbWZhfrmIHQjdTBrQ6LrFXZ0nfWOtE9k53C
lyXGC8zJPRGDxAlepdHGAmn7cxV7LvuzFvxowDfcZilsCI2mhjeTdzA+VZR9vRcC
dKkgdZ3oYTwcau0UXgP+nRIuRWpn3hBeJFFRl7j0DUttH3O776SLnQ8nuC9cwN2Q
6NWBw4SzjSWOBMKeTAW7uYbi+yhQtKI4g7jIt2cJFaSwygEP8XUfX80bDwWRHkxi
dUr4ZWxEsO4J4Ti4Gr/l4Sx0MqJ697YMT85+hzD5KSNaYdToYvEqOzgINT9Nv9FZ
Qmk2BHuMM7JS7Xl3Z5VD3NUCAwEAAQ==
-----END PUBLIC KEY-----
EOF
}

# Detect OS
OS="$(uname -s)"
case "$OS" in
  Darwin) OS="apple-darwin" ;;
  Linux)  OS="unknown-linux-gnu" ;;
  *)
    fail "unsupported OS: $OS"
    ;;
esac

# Detect arch
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64)         ARCH="x86_64" ;;
  aarch64|arm64)  ARCH="aarch64" ;;
  *)
    fail "unsupported architecture: $ARCH"
    ;;
esac

TARGET="${ARCH}-${OS}"
TARBALL="${BINARY}-${TARGET}.tar.gz"
URL="${S3_BASE_URL}/${TARBALL}"
CHECKSUMS_URL="${S3_BASE_URL}/${CHECKSUMS_FILE}"
SIGNATURE_URL="${S3_BASE_URL}/${SIGNATURE_FILE}"

# Resolve install dir
if [ -n "${INSTALL_DIR:-}" ]; then
  BIN_DIR="$INSTALL_DIR"
elif [ -w "/usr/local/bin" ]; then
  BIN_DIR="/usr/local/bin"
else
  BIN_DIR="$HOME/.local/bin"
fi
mkdir -p "$BIN_DIR"

require_command tar
require_command openssl
require_command awk

echo "Downloading $BINARY for $TARGET..."
echo "  tarball:   $URL"
echo "  manifest:  $CHECKSUMS_URL"
echo "  signature: $SIGNATURE_URL"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

download_file "$URL" "$TMPDIR/$TARBALL"
download_file "$CHECKSUMS_URL" "$TMPDIR/$CHECKSUMS_FILE"
download_file "$SIGNATURE_URL" "$TMPDIR/$SIGNATURE_FILE"

if [ -n "${BRAIN3_RELEASE_SIGNING_PUBLIC_KEY_FILE:-}" ]; then
  if [ ! -f "$BRAIN3_RELEASE_SIGNING_PUBLIC_KEY_FILE" ]; then
    fail "release signing public key file not found: $BRAIN3_RELEASE_SIGNING_PUBLIC_KEY_FILE"
  fi
  PUBLIC_KEY_FILE="$BRAIN3_RELEASE_SIGNING_PUBLIC_KEY_FILE"
else
  PUBLIC_KEY_FILE="$TMPDIR/release-signing-public-key.pem"
  write_embedded_public_key "$PUBLIC_KEY_FILE"
fi

if ! openssl dgst -sha256 \
  -verify "$PUBLIC_KEY_FILE" \
  -signature "$TMPDIR/$SIGNATURE_FILE" \
  "$TMPDIR/$CHECKSUMS_FILE" >/dev/null 2>&1; then
  fail "release manifest signature verification failed"
fi

EXPECTED_SUM="$(awk -v tarball="$TARBALL" '
  $NF == tarball || $NF ~ ("/" tarball "$") { print $1; exit }
' "$TMPDIR/$CHECKSUMS_FILE")"

if [ -z "$EXPECTED_SUM" ]; then
  fail "signed manifest does not contain a checksum for $TARBALL"
fi

ACTUAL_SUM="$(openssl dgst -sha256 "$TMPDIR/$TARBALL" | awk '{print $NF}')"
if [ -z "$ACTUAL_SUM" ]; then
  fail "failed to compute checksum for $TARBALL"
fi

if [ "$ACTUAL_SUM" != "$EXPECTED_SUM" ]; then
  fail "checksum verification failed for $TARBALL"
fi

echo "Verified signed manifest and checksum for $TARBALL."

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
