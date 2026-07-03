# Release Candidate Artifacts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Do not use subagents for this repository. Do not commit; the user will commit.

**Goal:** Allow tags like `v0.2.10-rc1` to build and publish reviewable release artifacts without overwriting the stable `releases/latest` channel.

**Architecture:** Keep the existing release workflow as the artifact producer, but teach the publish path to distinguish stable SemVer tags from prerelease tags. Stable tags continue to update `releases/latest`; prerelease tags upload only to their exact version prefix and create a GitHub prerelease.

**Tech Stack:** GitHub Actions, bash release scripts, Rust/Cargo package versioning, S3 release layout.

---

## Current Behavior And Answer

Yes, a tag named `v0.2.10-rc1` matches the existing release trigger:

```yaml
on:
  push:
    tags:
      - "v*"
```

But it will not pass the current workflow unless `apps/gateway/Cargo.toml` also has:

```toml
version = "0.2.10-rc1"
```

The workflow currently validates exact equality:

```bash
if [ "v${APP_VERSION}" != "${GITHUB_REF_NAME}" ]; then
  echo "Tag ${GITHUB_REF_NAME} does not match apps/gateway version v${APP_VERSION}" >&2
  exit 1
fi
```

The unsafe part is S3 publishing. `scripts/upload-to-s3.sh` always updates `releases/latest`, so an RC tag would replace the stable latest manifest, install script, and tarballs:

```bash
upload_file "$src" "releases/latest/$TARBALL"
upload_required_metadata "$TARBALLS_DIR/SHA256SUMS" "SHA256SUMS"
upload_required_metadata "$TARBALLS_DIR/SHA256SUMS.sig" "SHA256SUMS.sig"
```

That means the implementation should not be "just push a fake tag" unless we intentionally want `latest` to point at the RC. The safer plan below creates RC artifacts while preserving stable latest.

## Files

- Modify: `.github/workflows/release.yml`
  - Detect prerelease tags.
  - Mark GitHub releases as prereleases for tags containing a SemVer prerelease suffix.
  - Pass a flag to the S3 uploader so RC tags do not update `releases/latest`.
- Modify: `scripts/upload-to-s3.sh`
  - Add `BRAIN3_UPDATE_LATEST=true|false`.
  - Upload every build to `releases/<tag>/...`.
  - Upload to `releases/latest/...` only when `BRAIN3_UPDATE_LATEST=true`.
- Modify: `scripts/test-release-signing.sh`
  - Add coverage for `BRAIN3_UPDATE_LATEST=false` if the current tests exercise upload script behavior. If this script only tests manifest signing, leave it unchanged.
- Modify during RC release prep only: `apps/gateway/Cargo.toml`
  - Set version to the prerelease version, for example `0.2.10-rc1`.
- Modify during RC release prep only, if Cargo updates it: `Cargo.lock`
  - Let `cargo check` or `cargo metadata` refresh the package version in the lockfile if needed.
- Optional docs update during RC release prep: `README.MD`
  - Do not update the user-facing stable install command for an RC unless the user explicitly wants to advertise that RC.

## Task 1: Make S3 Latest Updates Explicit

**Files:**
- Modify: `scripts/upload-to-s3.sh`

- [ ] **Step 1: Add an environment flag near `AWS_REGION`**

Insert this after:

```bash
AWS_REGION="${AWS_REGION:-us-east-1}"
```

Add:

```bash
BRAIN3_UPDATE_LATEST="${BRAIN3_UPDATE_LATEST:-true}"
case "$BRAIN3_UPDATE_LATEST" in
  true|false) ;;
  *)
    echo "Error: BRAIN3_UPDATE_LATEST must be true or false, got: $BRAIN3_UPDATE_LATEST" >&2
    exit 1
    ;;
esac
```

- [ ] **Step 2: Gate tarball uploads to `releases/latest`**

Replace this block:

```bash
upload_file "$SRC" "releases/$VERSION/$TARBALL"
upload_file "$SRC" "releases/latest/$TARBALL"
```

With:

```bash
upload_file "$SRC" "releases/$VERSION/$TARBALL"
if [ "$BRAIN3_UPDATE_LATEST" = "true" ]; then
  upload_file "$SRC" "releases/latest/$TARBALL"
fi
```

- [ ] **Step 3: Gate metadata uploads to `releases/latest`**

Replace `upload_required_metadata()` with:

```bash
upload_required_metadata() {
  local src="$1"
  local name="$2"

  if [ ! -f "$src" ]; then
    echo "Error: required release metadata file not found: $src" >&2
    exit 1
  fi

  upload_file "$src" "releases/$VERSION/$name"
  if [ "$BRAIN3_UPDATE_LATEST" = "true" ]; then
    upload_file "$src" "releases/latest/$name"
  fi
}
```

- [ ] **Step 4: Gate latest install script upload**

Replace:

```bash
# latest copy: always points at releases/latest
STAMPED_LATEST="$(mktemp)"
sed "s|__BUCKET__|$BUCKET|g" "$SCRIPT_DIR/install.sh" > "$STAMPED_LATEST"
aws s3 cp "$STAMPED_LATEST" "s3://$BUCKET/releases/latest/install.sh" --region "$AWS_REGION"
rm -f "$STAMPED_LATEST"
```

With:

```bash
if [ "$BRAIN3_UPDATE_LATEST" = "true" ]; then
  STAMPED_LATEST="$(mktemp)"
  sed "s|__BUCKET__|$BUCKET|g" "$SCRIPT_DIR/install.sh" > "$STAMPED_LATEST"
  aws s3 cp "$STAMPED_LATEST" "s3://$BUCKET/releases/latest/install.sh" --region "$AWS_REGION"
  rm -f "$STAMPED_LATEST"
fi
```

- [ ] **Step 5: Make the final install output version-aware**

Replace:

```bash
echo "Done. One-line install command:"
echo "  curl -sSfL https://$BUCKET.s3.amazonaws.com/releases/latest/install.sh | sh"
```

With:

```bash
echo "Done. One-line install command:"
if [ "$BRAIN3_UPDATE_LATEST" = "true" ]; then
  echo "  curl -sSfL https://$BUCKET.s3.amazonaws.com/releases/latest/install.sh | sh"
else
  echo "  curl -sSfL https://$BUCKET.s3.amazonaws.com/releases/$VERSION/install.sh | sh"
fi
```

- [ ] **Step 6: Verify script syntax**

Run:

```bash
bash -n scripts/upload-to-s3.sh
```

Expected: exit 0 with no output.

## Task 2: Detect Prerelease Tags In The Release Workflow

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Add a release metadata step after tag validation**

Insert after `Validate release tag matches apps/gateway version`:

```yaml
      - name: Classify release tag
        id: release_meta
        run: |
          if [[ "${GITHUB_REF_NAME}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            echo "prerelease=false" >> "$GITHUB_OUTPUT"
            echo "update_latest=true" >> "$GITHUB_OUTPUT"
          elif [[ "${GITHUB_REF_NAME}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-.+ ]]; then
            echo "prerelease=true" >> "$GITHUB_OUTPUT"
            echo "update_latest=false" >> "$GITHUB_OUTPUT"
          else
            echo "Unsupported release tag format: ${GITHUB_REF_NAME}" >&2
            exit 1
          fi
```

- [ ] **Step 2: Expose release metadata from the build job**

Add outputs to the `build` job:

```yaml
    outputs:
      prerelease: ${{ steps.release_meta.outputs.prerelease }}
      update_latest: ${{ steps.release_meta.outputs.update_latest }}
```

Note: this requires the `release_meta` step to run in every matrix row. The output value is deterministic for all rows because it depends only on `GITHUB_REF_NAME`.

- [ ] **Step 3: Mark GitHub RC releases as prereleases**

In the `softprops/action-gh-release@v2` step, add:

```yaml
          prerelease: ${{ needs.build.outputs.prerelease == 'true' }}
```

The final block should look like:

```yaml
      - uses: softprops/action-gh-release@v2
        with:
          files: artifacts/*
          generate_release_notes: true
          prerelease: ${{ needs.build.outputs.prerelease == 'true' }}
```

- [ ] **Step 4: Pass the latest-update flag to S3 publishing**

In the `Upload to S3` step, add:

```yaml
          BRAIN3_UPDATE_LATEST: ${{ needs.build.outputs.update_latest }}
```

The env block should look like:

```yaml
        env:
          AWS_REGION: ${{ secrets.AWS_REGION || 'us-east-1' }}
          BRAIN3_UPDATE_LATEST: ${{ needs.build.outputs.update_latest }}
```

- [ ] **Step 5: Verify workflow YAML parses**

Run:

```bash
ruby -e 'require "yaml"; YAML.load_file(".github/workflows/release.yml"); puts "ok"'
```

Expected:

```text
ok
```

## Task 3: Prepare The RC Version

**Files:**
- Modify: `apps/gateway/Cargo.toml`
- Modify: `Cargo.lock` only if Cargo refreshes it
- Do not modify `README.MD` for RC unless explicitly requested

- [ ] **Step 1: Set the gateway version to the RC**

Change:

```toml
version = "0.2.9"
```

To:

```toml
version = "0.2.10-rc1"
```

- [ ] **Step 2: Refresh Cargo metadata if needed**

Run:

```bash
cargo check -p brain3
```

Expected: exit 0. If `Cargo.lock` changes only for the `brain3` package version, include it in the RC prep changes.

- [ ] **Step 3: Verify the tag validation condition locally**

Run:

```bash
APP_VERSION=$(awk -F '"' '/^version = / {print $2; exit}' apps/gateway/Cargo.toml)
test "v${APP_VERSION}" = "v0.2.10-rc1"
```

Expected: exit 0.

## Task 4: Full Local Verification

**Files:**
- No edits expected

- [ ] **Step 1: Compile test targets**

Run:

```bash
cargo test -p brain3 --no-run
```

Expected: exit 0.

- [ ] **Step 2: Run Rust tests**

Run:

```bash
cargo test
```

Expected: exit 0.

- [ ] **Step 3: Verify upload script syntax**

Run:

```bash
bash -n scripts/upload-to-s3.sh
```

Expected: exit 0.

- [ ] **Step 4: Verify workflow YAML parses**

Run:

```bash
ruby -e 'require "yaml"; YAML.load_file(".github/workflows/release.yml"); puts "ok"'
```

Expected:

```text
ok
```

## Task 5: Create And Push The RC Tag

**Files:**
- No file edits

- [ ] **Step 1: Confirm the working tree contains only intended RC changes**

Run:

```bash
git status --short
```

Expected: only the workflow/script changes and RC version bump files are listed.

- [ ] **Step 2: The user commits the RC changes**

The agent must not commit. The user should commit the approved changes.

- [ ] **Step 3: The user creates the RC tag**

Run after the commit exists:

```bash
git tag v0.2.10-rc1
```

- [ ] **Step 4: The user pushes the RC tag**

Run:

```bash
git push origin v0.2.10-rc1
```

Expected: GitHub Actions starts the `Release` workflow.

## Task 6: Review The RC Artifacts

**Files:**
- No file edits

- [ ] **Step 1: Confirm the GitHub release is a prerelease**

Check the `v0.2.10-rc1` GitHub release.

Expected:
- It is marked as a prerelease.
- It includes the signed manifest files.
- It includes all tarballs, including `brain3-x86_64-pc-windows-msvc.tar.gz`.

- [ ] **Step 2: Confirm S3 versioned artifacts exist**

Expected keys:

```text
releases/v0.2.10-rc1/brain3-x86_64-apple-darwin.tar.gz
releases/v0.2.10-rc1/brain3-aarch64-apple-darwin.tar.gz
releases/v0.2.10-rc1/brain3-x86_64-unknown-linux-gnu.tar.gz
releases/v0.2.10-rc1/brain3-aarch64-unknown-linux-gnu.tar.gz
releases/v0.2.10-rc1/brain3-x86_64-pc-windows-msvc.tar.gz
releases/v0.2.10-rc1/SHA256SUMS
releases/v0.2.10-rc1/SHA256SUMS.sig
releases/v0.2.10-rc1/install.sh
```

- [ ] **Step 3: Confirm stable latest did not move**

Check that these keys were not overwritten by the RC workflow:

```text
releases/latest/install.sh
releases/latest/SHA256SUMS
releases/latest/SHA256SUMS.sig
```

Expected: they still correspond to the previous stable release.

## Task 7: Stable Release Follow-Up

**Files:**
- Modify during stable release prep: `apps/gateway/Cargo.toml`
- Modify during stable release prep: `Cargo.lock` if Cargo refreshes it
- Modify during stable release prep: `README.MD` if updating the documented stable install URL

- [ ] **Step 1: Promote version from RC to stable**

Change:

```toml
version = "0.2.10-rc1"
```

To:

```toml
version = "0.2.10"
```

- [ ] **Step 2: Run normal verification**

Run:

```bash
cargo test -p brain3 --no-run
cargo test
```

Expected: both exit 0.

- [ ] **Step 3: The user commits and tags the stable release**

The agent must not commit.

```bash
git tag v0.2.10
git push origin v0.2.10
```

Expected: stable release updates both `releases/v0.2.10/...` and `releases/latest/...`.

## Review Notes

- This plan preserves the closed preregistered-client OAuth policy. It does not change auth, ingress, MCP behavior, or threat model boundaries.
- This plan creates release artifacts through the real release pipeline, not a separate fake-artifact path.
- The RC tag is not really "fake"; it must be a valid prerelease version in `apps/gateway/Cargo.toml` because the workflow intentionally prevents tag/version mismatches.
- The most important safety behavior is preventing RC tags from updating `releases/latest`.
