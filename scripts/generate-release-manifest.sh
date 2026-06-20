#!/usr/bin/env bash
set -euo pipefail

ARTIFACTS_DIR="${1:-}"

if [ -z "$ARTIFACTS_DIR" ]; then
  echo "Usage: $0 <artifacts-dir>" >&2
  exit 1
fi

if [ ! -d "$ARTIFACTS_DIR" ]; then
  echo "Error: artifacts directory not found: $ARTIFACTS_DIR" >&2
  exit 1
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "Error: openssl not found" >&2
  exit 1
fi

RELEASE_SIGNING_KEY_FILE="${RELEASE_SIGNING_KEY_FILE:-}"
if [ -z "$RELEASE_SIGNING_KEY_FILE" ]; then
  echo "Error: RELEASE_SIGNING_KEY_FILE is required" >&2
  exit 1
fi

if [ ! -f "$RELEASE_SIGNING_KEY_FILE" ]; then
  echo "Error: signing key not found: $RELEASE_SIGNING_KEY_FILE" >&2
  exit 1
fi

CHECKSUM_FILE="$ARTIFACTS_DIR/SHA256SUMS"
SIGNATURE_FILE="$ARTIFACTS_DIR/SHA256SUMS.sig"

checksum_cmd() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$@"
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$@"
  else
    echo "Error: neither sha256sum nor shasum found" >&2
    exit 1
  fi
}

tarballs=()
while IFS= read -r tarball; do
  tarballs+=("$tarball")
done < <(find "$ARTIFACTS_DIR" -maxdepth 1 -type f -name 'brain3-*.tar.gz' -print | sort)

if [ "${#tarballs[@]}" -eq 0 ]; then
  echo "Error: no release tarballs found in $ARTIFACTS_DIR" >&2
  exit 1
fi

checksum_cmd "${tarballs[@]}" > "$CHECKSUM_FILE"

openssl dgst -sha256 \
  -sign "$RELEASE_SIGNING_KEY_FILE" \
  -out "$SIGNATURE_FILE" \
  "$CHECKSUM_FILE"

echo "Generated:"
echo "  $CHECKSUM_FILE"
echo "  $SIGNATURE_FILE"
