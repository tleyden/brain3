# Release Signing Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generate and publish signed checksum metadata for Brain3 release tarballs without changing installer verification behavior yet.

**Architecture:** Add one release-manifest generation script, call it from the GitHub release workflow after all tarballs are collected, and publish the resulting manifest/signature through the existing GitHub Release and S3 paths. Keep `install.sh` behavior unchanged in this phase.

**Tech Stack:** GitHub Actions YAML, POSIX/Bash shell, OpenSSL, AWS CLI

---

### Task 1: Add Release Manifest Generation Script

**Files:**
- Create: `scripts/generate-release-manifest.sh`

- [ ] **Step 1: Create a script that discovers release tarballs and computes checksums**

The script should:

- accept one argument: artifacts directory
- require `RELEASE_SIGNING_KEY_FILE`
- find `brain3-*.tar.gz`
- write a sorted `SHA256SUMS` file into the artifacts directory

- [ ] **Step 2: Add detached signature generation**

Use:

```bash
openssl dgst -sha256 \
  -sign "$RELEASE_SIGNING_KEY_FILE" \
  -out "$ARTIFACTS_DIR/SHA256SUMS.sig" \
  "$ARTIFACTS_DIR/SHA256SUMS"
```

- [ ] **Step 3: Fail early on missing inputs**

The script should exit non-zero if:

- no artifacts directory is provided
- the directory does not exist
- no tarballs match
- `openssl` is unavailable
- `RELEASE_SIGNING_KEY_FILE` is unset or missing

### Task 2: Wire Manifest Generation Into The Release Workflow

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Add a prepare-assets job after build**

The job should:

- download the built tarballs into `artifacts/`
- check out the repo to access scripts
- decode `secrets.BRAIN3_RELEASE_SIGNING_KEY_PEM_B64` into a temporary PEM file
- run `bash scripts/generate-release-manifest.sh artifacts`
- upload the resulting combined assets as a workflow artifact

- [ ] **Step 2: Make release publishing consume prepared assets**

The GitHub Release job should:

- depend on `prepare-assets`
- download the prepared assets artifact
- publish all files in `artifacts/`

- [ ] **Step 3: Make S3 publishing consume prepared assets**

The S3 publish job should:

- depend on `prepare-assets`
- download the prepared assets artifact
- run the existing uploader against that directory

### Task 3: Publish Manifest And Signature To S3

**Files:**
- Modify: `scripts/upload-to-s3.sh`

- [ ] **Step 1: Extend the uploader to copy metadata files when present**

Upload:

- `SHA256SUMS`
- `SHA256SUMS.sig`

to both:

- `releases/$VERSION/`
- `releases/latest/`

- [ ] **Step 2: Warn clearly if metadata files are missing**

Do not silently skip them in release automation. Emit a warning so local/manual runs show the gap immediately.

### Task 4: Update README To Reflect The New State

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Keep the install command latest-only**

Retain:

```bash
curl -sSfL https://brain3.s3.amazonaws.com/releases/latest/install.sh | sh
```

- [ ] **Step 2: Update the security wording**

Replace the statement that release assets are "not yet signed or checksummed" with wording that says signed checksum metadata is published, but the installer does not yet enforce validation automatically.

### Task 5: Verify The Phase 1 Plumbing

**Files:**
- No repository file changes required

- [ ] **Step 1: Run shell syntax checks**

Run:

```bash
bash -n scripts/generate-release-manifest.sh
bash -n scripts/upload-to-s3.sh
```

- [ ] **Step 2: Run a local manifest-generation smoke test**

Generate a temporary signing key, create a temporary fake tarball, run the script, and verify `SHA256SUMS` plus `SHA256SUMS.sig` are created.

- [ ] **Step 3: Run repository verification**

Run:

```bash
cargo test
```
