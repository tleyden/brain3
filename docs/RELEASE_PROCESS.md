# Release Process (AI Generated)

## Prerequisites

- `gh` CLI installed and authenticated (`gh auth login`)
- You are on the `main` branch with a clean working tree
- All milestone issues for the release are closed

## One-time GitHub secrets setup

These secrets must be set once per repo. The AWS secrets are used to publish release artifacts to S3, and the signing key secret is used by the release workflow to sign `SHA256SUMS`.

```bash
gh secret set AWS_ACCESS_KEY_ID              --repo tleyden/brain3 --body "AKIA..."
gh secret set AWS_SECRET_ACCESS_KEY          --repo tleyden/brain3 --body "..."
gh secret set BRAIN3_S3_BUCKET               --repo tleyden/brain3 --body "your-bucket-name"
gh secret set AWS_REGION                     --repo tleyden/brain3 --body "us-east-1"
gh secret set BRAIN3_RELEASE_SIGNING_KEY_PEM_B64 --repo tleyden/brain3 < brain3-release-signing-key.pem.b64
```

Verify they are set:

```bash
gh secret list --repo tleyden/brain3
```

## 1. Prepare release notes from a GitHub milestone

Create a milestone in advance (e.g. `v0.1.6`) and assign issues and PRs to it as you work.

When ready to release, generate a notes draft from the closed milestone issues:

```bash
VERSION=v0.1.6

gh issue list \
  --milestone "$VERSION" \
  --state closed \
  --json number,title,url \
  --jq '.[] | "- \(.title) (\(.url))"'
```

Copy the output into a file for the next step, or pipe it directly:

```bash
gh issue list \
  --milestone "$VERSION" \
  --state closed \
  --json number,title,url \
  --jq '.[] | "- \(.title) (\(.url))"' \
  > /tmp/release-notes.md
```

Edit `/tmp/release-notes.md` to add a short summary paragraph at the top.

## 2. Tag and push

```bash
VERSION=v0.1.6

git tag -a "$VERSION" -m "Release $VERSION"
git push origin "$VERSION"
```

Pushing the tag triggers `.github/workflows/release.yml`, which:

1. Builds `brain3` for all four targets in parallel:
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu`
   - `x86_64-apple-darwin`
   - `aarch64-apple-darwin`
2. Generates and signs `SHA256SUMS` using `BRAIN3_RELEASE_SIGNING_KEY_PEM_B64`.
3. Publishes the tarballs plus `SHA256SUMS` and `SHA256SUMS.sig` to GitHub Releases and S3.

The workflow takes ~5–10 minutes. Monitor it:

```bash
gh run watch
```

## 3. Attach your release notes

Once the workflow completes, edit the release to replace the auto-generated notes with your draft:

```bash
VERSION=v0.1.6

gh release edit "$VERSION" \
  --notes-file /tmp/release-notes.md
```

Or edit in the browser:

```bash
gh release view "$VERSION" --web
```

## 4. Verify the release

```bash
VERSION=v0.1.6

# List attached assets
gh release view "$VERSION"

# Download and smoke-test the Linux binary
gh release download "$VERSION" \
  --pattern "brain3-x86_64-unknown-linux-gnu.tar.gz" \
  --dir /tmp/brain3-test

tar -xzf /tmp/brain3-test/brain3-x86_64-unknown-linux-gnu.tar.gz -C /tmp/brain3-test
/tmp/brain3-test/brain3 --help
```

## Manual S3 Upload (for testing without tagging)

Use this when you want to push a binary and `install.sh` to S3 without creating a GitHub release or tag.

### Prerequisites

- AWS CLI configured (`aws configure`) with credentials that have `s3:PutObject` on the bucket
- `BRAIN3_S3_BUCKET` env var set, or pass the bucket name as an argument

### Steps

**1. Detect your target triple:**

```bash
rustc -vV | grep host | awk '{print $2}'
# e.g. aarch64-apple-darwin
```

**2. Build and package for your local platform only:**

```bash
mkdir -p /tmp/brain3-artifacts
cargo build --release
tar -czf /tmp/brain3-artifacts/brain3-$(rustc -vV | awk '/host:/ {print $2}').tar.gz -C target/release brain3
```

Cross-compiling all four targets locally is complex — if you need all platforms, push a branch and let the PR workflow build them.

**3. Manually generate release signatures:**

```bash
./scripts/generate-release-manifest.sh
mkdir -p /tmp/brain3-artifacts
cargo build --release
tar -czf /tmp/brain3-artifacts/brain3-$(rustc -vV | awk '/host:/ {print $2}').tar.gz -C target/release brain3
RELEASE_SIGNING_KEY_FILE=.secrets/brain3-release-signing-key.pem ./scripts/generate-release-manifest.sh /tmp/brain3-artifacts
```

This creates:

- `/tmp/brain3-artifacts/SHA256SUMS`
- `/tmp/brain3-artifacts/SHA256SUMS.sig`

**4. Upload to S3:**

```bash
# Uploads to releases/dev/ and releases/latest/ by default
bash scripts/upload-to-s3.sh <bucket-name> dev /tmp/brain3-artifacts

# Or specify a custom version label
bash scripts/upload-to-s3.sh <bucket-name> v0.1.6-rc1 /tmp/brain3-artifacts
```

The script uploads each tarball it finds to both `releases/<version>/` and `releases/latest/`,
uploads `SHA256SUMS` and `SHA256SUMS.sig`, and also uploads `scripts/install.sh` to both the versioned and `latest` S3 paths.

**5. Test the install script against your uploaded artifacts:**

```bash
S3_BASE_URL="https://<bucket>.s3.amazonaws.com/releases/latest" \
  bash scripts/install.sh
```
