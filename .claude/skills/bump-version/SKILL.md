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

---

## Step 1 — Bump version in files

Run the bump script (it detects the current version from `apps/gateway/Cargo.toml`, applies all replacements, and refreshes `Cargo.lock`):

```bash
bash .claude/skills/bump-version/bump.sh VERSION
```

Then run `cargo test` to verify nothing is broken.

Do NOT commit — tell the user to commit themselves, as per project instructions.

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

---

## Gotchas

- The version argument can have or omit a leading `v` — both `0.2.3` and `v0.2.3` work.
- Push only the tag (`git push origin "$VERSION"`), not all branches — pushing everything can trigger unintended workflows.
- Tag and push **after** the user has committed the version bump.
- `cargo fetch` (not `cargo build`) is used to update `Cargo.lock` quickly.
