---
name: bump-version
description: Bump the release version everywhere in the Brain3 project — Cargo.toml, pyproject.toml, README, first_run_setup.rs, and test fixtures — then execute the full release process: tag, CI workflow, and verification.
---

# bump-version / release

Full release process for Brain3. Execute each step using the Bash tool — do not show commands for the user to copy-paste, run them directly.

## Files updated by the bump script

| File | What changes |
|------|-------------|
| `apps/gateway/Cargo.toml` | `version = "X.Y.Z"` |
| `brain3-mcp-vault-tools/pyproject.toml` | `version = "X.Y.Z"` |
| `README.MD` | S3 install URL (`/releases/vX.Y.Z/install.sh`) |
| `crates/core/src/application/first_run_setup.rs` | `CURRENT_RELEASE = "vX.Y.Z"` |
| `brain3-mcp-vault-tools/tests/test_server_startup.py` | Two version string fixtures |
| `Cargo.lock` | Auto-updated via `cargo fetch` |
| `brain3-mcp-vault-tools/uv.lock` | Regenerated via `uv lock --project ./brain3-mcp-vault-tools` |

---

## Step 0 — Ensure we're on a release branch

Before bumping anything, check the current branch:

```bash
git rev-parse --abbrev-ref HEAD
```

The version bump must happen on a dedicated release branch named `bump_version_<XYZ>` (the version with dots removed — e.g. `0.2.8` → `bump_version_028`), **not** on `main`.

- If already on the expected `bump_version_<XYZ>` branch, proceed to Step 1.
- Otherwise (on `main` or any other branch), **stop and offer to create the branch** for the user. Do not create it without confirmation. Once confirmed, run:

```bash
git checkout -b bump_version_<XYZ>
```

Only after we're on the release branch should you continue to Step 1.

---

## Step 1 — Bump version in files

Run the bump script (it detects the current version from `apps/gateway/Cargo.toml`, applies all replacements, and refreshes `Cargo.lock`):

```bash
bash .claude/skills/bump-version/bump.sh VERSION
```

Then refresh and verify the Python lockfile (pyproject.toml was just bumped, so uv.lock will be stale):

```bash
uv lock --project ./brain3-mcp-vault-tools
uv lock --project ./brain3-mcp-vault-tools --check
```

Then run `cargo test` to verify nothing is broken.

Do NOT commit — tell the user to commit themselves, as per project instructions. Remind them that both `brain3-mcp-vault-tools/pyproject.toml` and `brain3-mcp-vault-tools/uv.lock` must be included in the commit.

---

## Step 2 — Tag and push (triggers CI release workflow)

Ask the user to confirm before tagging and pushing. Once confirmed, run:

```bash
VERSION=vX.Y.Z
git tag -a "$VERSION" -m "Release $VERSION"
git push origin "$VERSION"
```

Pushing the tag triggers `.github/workflows/release.yml`, which builds all four targets, signs `SHA256SUMS`, and publishes to GitHub Releases and S3 (~5–10 min).

Then monitor progress by running:

```bash
gh run watch
```

---

## Step 3 — Verify

Once the workflow completes, run:

```bash
VERSION=vX.Y.Z
gh release view "$VERSION"
```

Report the listed assets to the user.

Then verify the Docker image published to GHCR by pulling it:

```bash
VERSION=vX.Y.Z
docker pull ghcr.io/tleyden/brain3-mcp-vault-tools:$VERSION
```

Confirm the pull succeeds and report the digest to the user.

---

## Gotchas

- The version argument can have or omit a leading `v` — both `0.2.3` and `v0.2.3` work.
- Push only the tag (`git push origin "$VERSION"`), not all branches — pushing everything can trigger unintended workflows.
- Tag and push **after** the user has committed the version bump.
- `cargo fetch` (not `cargo build`) is used to update `Cargo.lock` quickly.
- Any change to `brain3-mcp-vault-tools/pyproject.toml` or other package metadata requires regenerating `brain3-mcp-vault-tools/uv.lock` in the same commit — `uv lock --project ./brain3-mcp-vault-tools --check` will fail in CI otherwise.
