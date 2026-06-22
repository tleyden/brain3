#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACTS_DIR="$(mktemp -d)"
INSTALL_OK_DIR="$(mktemp -d)"
INSTALL_FAIL_DIR="$(mktemp -d)"
TMP_KEY_DIR="$(mktemp -d)"
SERVER_LOG="$(mktemp)"
SERVER_PID=""
cleanup() {
  if [ -n "$SERVER_PID" ]; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$ARTIFACTS_DIR" "$INSTALL_OK_DIR" "$INSTALL_FAIL_DIR" "$TMP_KEY_DIR"
  rm -f "$SERVER_LOG"
}
trap cleanup EXIT

fail() {
  echo "Error: $*" >&2
  exit 1
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    fail "required command not found: $1"
  fi
}

require_command openssl
require_command tar
require_command python3

os="$(uname -s)"
case "$os" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-gnu" ;;
  *) fail "unsupported OS for smoke test: $os" ;;
esac

arch="$(uname -m)"
case "$arch" in
  x86_64) arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) fail "unsupported architecture for smoke test: $arch" ;;
esac

target="${arch}-${os}"
tarball="brain3-${target}.tar.gz"
private_key="$TMP_KEY_DIR/release-signing-key.pem"
public_key="$TMP_KEY_DIR/release-signing-public-key.pem"

cat > "$ARTIFACTS_DIR/brain3" <<'EOF'
#!/usr/bin/env sh
if [ "${1:-}" = "--version" ]; then
  echo "brain3 smoke-test"
else
  echo "brain3 smoke-test"
fi
EOF
chmod +x "$ARTIFACTS_DIR/brain3"
tar -czf "$ARTIFACTS_DIR/$tarball" -C "$ARTIFACTS_DIR" brain3
rm -f "$ARTIFACTS_DIR/brain3"

openssl genrsa -out "$private_key" 2048 >/dev/null 2>&1
openssl pkey -in "$private_key" -pubout -out "$public_key" >/dev/null 2>&1

RELEASE_SIGNING_KEY_FILE="$private_key" \
  bash "$ROOT_DIR/scripts/generate-release-manifest.sh" "$ARTIFACTS_DIR"

test -s "$ARTIFACTS_DIR/SHA256SUMS"
test -s "$ARTIFACTS_DIR/SHA256SUMS.sig"

openssl dgst -sha256 \
  -verify "$public_key" \
  -signature "$ARTIFACTS_DIR/SHA256SUMS.sig" \
  "$ARTIFACTS_DIR/SHA256SUMS" >/dev/null

port="$(python3 - <<'PY'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"

python3 -m http.server "$port" --bind 127.0.0.1 --directory "$ARTIFACTS_DIR" > "$SERVER_LOG" 2>&1 &
SERVER_PID="$!"
sleep 1

INSTALL_DIR="$INSTALL_OK_DIR" \
S3_BASE_URL="http://127.0.0.1:$port" \
BRAIN3_RELEASE_SIGNING_PUBLIC_KEY_FILE="$public_key" \
sh "$ROOT_DIR/scripts/install.sh"

test -x "$INSTALL_OK_DIR/brain3"

printf 'tamper\n' >> "$ARTIFACTS_DIR/$tarball"

if INSTALL_DIR="$INSTALL_FAIL_DIR" \
  S3_BASE_URL="http://127.0.0.1:$port" \
  BRAIN3_RELEASE_SIGNING_PUBLIC_KEY_FILE="$public_key" \
  sh "$ROOT_DIR/scripts/install.sh"; then
  fail "installer succeeded after tarball tampering"
fi

if [ -e "$INSTALL_FAIL_DIR/brain3" ]; then
  fail "installer wrote a binary after tarball tampering"
fi

echo "Release signing smoke test passed."
