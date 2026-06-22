#!/usr/bin/env bash
set -euo pipefail

# Upload brain3 release tarballs to S3.
#
# Usage:
#   ./scripts/upload-to-s3.sh <bucket> [version] [tarballs-dir]
#
# Arguments:
#   bucket        S3 bucket name (required)
#   version       release version, e.g. v0.1.0 (default: current git tag)
#   tarballs-dir  directory containing the .tar.gz files (default: current dir)
#
# AWS credentials are read from the environment or the AWS CLI config:
#   AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_REGION (optional, default: us-east-1)

BINARY="brain3"
TARGETS=(
  "x86_64-apple-darwin"
  "aarch64-apple-darwin"
  "x86_64-unknown-linux-gnu"
  "aarch64-unknown-linux-gnu"
)

BUCKET="${1:-}"
if [ -z "$BUCKET" ]; then
  echo "Usage: $0 <bucket> [version] [tarballs-dir]" >&2
  exit 1
fi

VERSION="${2:-$(git describe --tags --exact-match 2>/dev/null || echo "dev")}"

TARBALLS_DIR="${3:-.}"

AWS_REGION="${AWS_REGION:-us-east-1}"

if ! command -v aws >/dev/null 2>&1; then
  echo "Error: aws CLI not found. Install it from https://docs.aws.amazon.com/cli/latest/userguide/install-cliv2.html" >&2
  exit 1
fi

upload_file() {
  local src="$1"
  local s3_key="$2"
  echo "  s3://$BUCKET/$s3_key"
  aws s3 cp "$src" "s3://$BUCKET/$s3_key" \
    --region "$AWS_REGION"
}

upload_required_metadata() {
  local src="$1"
  local name="$2"

  if [ ! -f "$src" ]; then
    echo "Error: required release metadata file not found: $src" >&2
    exit 1
  fi

  upload_file "$src" "releases/$VERSION/$name"
  upload_file "$src" "releases/latest/$name"
}

echo "Uploading $BINARY $VERSION to s3://$BUCKET"
echo ""

for TARGET in "${TARGETS[@]}"; do
  TARBALL="${BINARY}-${TARGET}.tar.gz"
  SRC="$TARBALLS_DIR/$TARBALL"

  if [ ! -f "$SRC" ]; then
    echo "Warning: $SRC not found, skipping." >&2
    continue
  fi

  echo "[$TARGET]"
  upload_file "$SRC" "releases/$VERSION/$TARBALL"
  upload_file "$SRC" "releases/latest/$TARBALL"
done

echo "[release metadata]"
upload_required_metadata "$TARBALLS_DIR/SHA256SUMS" "SHA256SUMS"
upload_required_metadata "$TARBALLS_DIR/SHA256SUMS.sig" "SHA256SUMS.sig"

# Also upload the install script itself to latest/ so users can:
#   curl https://<bucket>.s3.amazonaws.com/releases/latest/install.sh | sh
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "$SCRIPT_DIR/install.sh" ]; then
  echo "[install.sh]"
  STAMPED="$(mktemp)"
  sed "s|__BUCKET__|$BUCKET|g" "$SCRIPT_DIR/install.sh" > "$STAMPED"
  aws s3 cp "$STAMPED" "s3://$BUCKET/releases/latest/install.sh" --region "$AWS_REGION"
  aws s3 cp "$STAMPED" "s3://$BUCKET/releases/$VERSION/install.sh" --region "$AWS_REGION"
  rm -f "$STAMPED"
fi

echo ""
echo "Done. One-line install command:"
echo "  curl -sSfL https://$BUCKET.s3.amazonaws.com/releases/latest/install.sh | sh"
