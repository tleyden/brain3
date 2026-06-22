Phase 1: Generate And Publish Integrity Metadata**
Goal: prove the release pipeline can produce and ship the right files.

Changes:
- [.github/workflows/release.yml](/Users/tleyden/Development/brain3_workspace2/.github/workflows/release.yml:1)
  - After packaging tarballs, generate `SHA256SUMS`.
  - Sign `SHA256SUMS` with a release private key stored in GitHub Actions secrets.
  - Publish:
    - `brain3-<target>.tar.gz`
    - `SHA256SUMS`
    - `SHA256SUMS.sig`
- [scripts/upload-to-s3.sh](/Users/tleyden/Development/brain3_workspace2/scripts/upload-to-s3.sh:1)
  - Upload manifest and signature alongside tarballs to:
    - `releases/<version>/...`
    - `releases/latest/...`
- [README.md](/Users/tleyden/Development/brain3_workspace2/README.md:96)
  - Keep only:
    ```bash
    curl -sSfL https://brain3.s3.amazonaws.com/releases/latest/install.sh | sh
    ```
  - Add one short note that signed release metadata is published with each release.
- `scripts/install.sh`
  - No verification logic yet.
  - Optionally print where `SHA256SUMS` and `SHA256SUMS.sig` live for manual verification, but no enforcement.

Outputs for a release:
- `releases/v0.1.8/brain3-x86_64-apple-darwin.tar.gz`
- `releases/v0.1.8/SHA256SUMS`
- `releases/v0.1.8/SHA256SUMS.sig`
- same three under `releases/latest/`

Key design choice:
- Sign the manifest, not each tarball individually.
- That keeps the release process simple and scales across targets.

**Phase 2: Manual Verification Support**
Goal: make the signatures usable before enforcing them.

Changes:
- Add a small documented verification flow in README for advanced users.
- Publish the release public key in-repo.
- Optionally add a helper script for local verification, but keep install behavior unchanged.

Result:
- You can confirm the signature flow works end-to-end without risking installer breakage.

**Phase 3: Enforce Verification In Installer**
Goal: actually remediate `M-11`.

Changes:
- [scripts/install.sh](/Users/tleyden/Development/brain3_workspace2/scripts/install.sh:1)
  - Download tarball, `SHA256SUMS`, and `SHA256SUMS.sig`.
  - Verify the signature using the embedded or bundled public key.
  - Verify the target tarball checksum against the signed manifest.
  - Abort on any mismatch before extract/install.
- README can still reference `latest` only if you want.
  - The security model then becomes: `latest/install.sh` fetches the newest release installer, and that installer verifies the exact artifacts it installs.
  - This is better than today, but still weaker than documenting a versioned installer URL.

**Phase 4: Tightening**
Optional hardening after the core fix works.

Changes:
- Add CI checks that fail if manifest/signature files are missing from release outputs.
- Add one end-to-end smoke test that tampers with a tarball and confirms future installer verification rejects it.
- Consider moving from ad hoc OpenSSL signing to a more structured signing format later if needed.

**Implementation Detail Recommendation**
For Phase 1, use a detached signature over `SHA256SUMS` with `openssl dgst -sha256 -sign ...`.

Why:
- Available in CI and common on macOS/Linux.
- Minimal moving parts.
- Good enough for this release pipeline.

Expected new files:
- `scripts/release-sign-manifest.sh` or equivalent small shell step
- public key file in repo for later phases
- new GitHub secret for the private signing key

**Tradeoff**
Keeping README on `latest` only is operationally simpler, but it means the default install command remains mutable at the URL layer. Phase 3 still improves this materially because the fetched installer will verify artifacts before install, but the strongest model would still be a versioned installer URL.