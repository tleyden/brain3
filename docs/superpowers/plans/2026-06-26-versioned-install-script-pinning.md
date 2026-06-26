# Plan: Pin Versioned install.sh to Its Own Version

## Problem

`/releases/v0.2.3/install.sh` and `/releases/v0.2.5/install.sh` are identical —
both download from `releases/latest/`. The root cause is in `scripts/upload-to-s3.sh`:

```bash
sed "s|__BUCKET__|$BUCKET|g" "$SCRIPT_DIR/install.sh" > "$STAMPED"
aws s3 cp "$STAMPED" "s3://$BUCKET/releases/latest/install.sh"
aws s3 cp "$STAMPED" "s3://$BUCKET/releases/$VERSION/install.sh"   # ← same file!
```

The sed command only stamps `__BUCKET__`, never the version. So the versioned
copy of `install.sh` still has:

```sh
S3_BASE_URL="${S3_BASE_URL:-https://__BUCKET__.s3.amazonaws.com/releases/latest}"
```

pointing at `latest`, regardless of which version directory it lives in.

## Fix (one file, one location)

In `scripts/upload-to-s3.sh`, replace the `install.sh` stamping block with two
separate stamped copies:

1. **latest copy** — stamp `__BUCKET__` only (keeps `releases/latest` in the URL,
   which is correct for the always-current alias).
2. **versioned copy** — stamp both `__BUCKET__` *and* `releases/latest` →
   `releases/$VERSION`, so the pinned script pulls exactly its own artifacts.

Concrete change (lines ~87–93 of `upload-to-s3.sh`):

```bash
if [ -f "$SCRIPT_DIR/install.sh" ]; then
  echo "[install.sh]"

  # latest copy: always points at releases/latest
  STAMPED_LATEST="$(mktemp)"
  sed "s|__BUCKET__|$BUCKET|g" "$SCRIPT_DIR/install.sh" > "$STAMPED_LATEST"
  aws s3 cp "$STAMPED_LATEST" "s3://$BUCKET/releases/latest/install.sh" --region "$AWS_REGION"
  rm -f "$STAMPED_LATEST"

  # versioned copy: points at releases/$VERSION so pinned installs stay pinned
  STAMPED_VERSIONED="$(mktemp)"
  sed -e "s|__BUCKET__|$BUCKET|g" \
      -e "s|releases/latest|releases/$VERSION|g" \
      "$SCRIPT_DIR/install.sh" > "$STAMPED_VERSIONED"
  aws s3 cp "$STAMPED_VERSIONED" "s3://$BUCKET/releases/$VERSION/install.sh" --region "$AWS_REGION"
  rm -f "$STAMPED_VERSIONED"
fi
```

No changes to `install.sh` itself or `release.yml`.

## Verification

After the next release tag is pushed:

1. `curl -sSfL https://brain3.s3.amazonaws.com/releases/vX.Y.Z/install.sh | grep S3_BASE_URL`
   should show `releases/vX.Y.Z`, not `releases/latest`.
2. Installing via the versioned URL should land the binary at exactly vX.Y.Z.
3. Installing via the `latest` URL should still install the newest version.

## Scope

One block in `scripts/upload-to-s3.sh`. No other files change.
