# Release Signing Phase 1 Design

**Goal:** Generate and publish signed release metadata for Brain3 artifacts without changing installer behavior yet.

## Scope

Phase 1 adds integrity metadata generation to the release pipeline:

- Generate `SHA256SUMS` for all release tarballs.
- Generate a detached signature `SHA256SUMS.sig` over that manifest.
- Publish both files to GitHub Releases and S3 versioned/latest paths.

Phase 1 does **not** add installer-side verification. `scripts/install.sh` remains unchanged for now.

## Approach

The release pipeline already builds all tarballs in GitHub Actions and publishes them to GitHub Releases and S3. The least disruptive change is to add a single signing step after the tarballs are gathered into one directory.

That step will:

1. Hash all `brain3-*.tar.gz` files into a sorted `SHA256SUMS` manifest.
2. Sign the manifest with `openssl dgst -sha256 -sign ...`.
3. Upload the manifest and detached signature wherever tarballs are already published.

Signing the manifest instead of each tarball keeps the format simple and avoids per-target signature management.

## Trust Model

This phase improves release provenance visibility but does not fully remediate the audit finding yet. The installer still trusts the fetched payload and does not validate the signed metadata automatically.

The resulting state is:

- Release artifacts have published checksums.
- The checksum manifest is signed in CI.
- Default installs still use `latest/install.sh`.
- Manual or future automated validation can rely on the published metadata.

## Files

- Add: `scripts/generate-release-manifest.sh`
- Add: `docs/superpowers/plans/2026-06-20-release-signing-phase1.md`
- Modify: `.github/workflows/release.yml`
- Modify: `scripts/upload-to-s3.sh`
- Modify: `README.md`

## Operational Notes

- GitHub Actions needs a new secret containing the private signing key, base64-encoded PEM.
- The signing key is used only in the release workflow after artifacts are built.
- S3 `latest/` remains a mutable convenience alias and mirrors the newest versioned release metadata.
