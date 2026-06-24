---
name: bump-version
description: Bump the release version everywhere in the Brain3 project — Cargo.toml, pyproject.toml, README, first_run_setup.rs, and test fixtures — then guide through the full release process: tag, CI workflow, release notes, and verification.
---

# bump-version / release

Full release process for Brain3. Start here on `main` with a clean working tree.

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

```bash
bash .claude/skills/bump-version/bump.sh 0.2.3
```

The script detects the current version from `apps/gateway/Cargo.toml`, applies all replacements, refreshes `Cargo.lock`, then asks before tagging. Answer `n` for now — commit first.

```bash
git commit -am "bump version 0.2.3"
```

---

## Step 2 — Generate release notes from the milestone

```bash
VERSION=v0.2.3

gh issue list \
  --milestone "$VERSION" \
  --state closed \
  --json number,title,url \
  --jq '.[] | "- \(.title) (\(.url))"' \
  > /tmp/release-notes.md
```

Edit `/tmp/release-notes.md` to add a short summary paragraph at the top.

---

## Step 3 — Tag and push (triggers CI release workflow)

```bash
VERSION=v0.2.3

git tag -a "$VERSION" -m "Release $VERSION"
git push origin "$VERSION"
```

Pushing the tag triggers `.github/workflows/release.yml`, which builds all four targets, signs `SHA256SUMS`, and publishes to GitHub Releases and S3 (~5–10 min).

Monitor progress:

```bash
gh run watch
```

---

## Step 4 — Attach release notes

Once the workflow completes:

```bash
VERSION=v0.2.3

gh release edit "$VERSION" --notes-file /tmp/release-notes.md
```

Or edit in the browser:

```bash
gh release view "$VERSION" --web
```

---

## Step 5 — Verify

```bash
VERSION=v0.2.3

# List attached assets
gh release view "$VERSION"

# Download and smoke-test the Linux binary
gh release download "$VERSION" \
  --pattern "brain3-x86_64-unknown-linux-gnu.tar.gz" \
  --dir /tmp/brain3-test

tar -xzf /tmp/brain3-test/brain3-x86_64-unknown-linux-gnu.tar.gz -C /tmp/brain3-test
/tmp/brain3-test/brain3 --help
```

---

## Gotchas

- The version argument can have or omit a leading `v` — both `0.2.3` and `v0.2.3` work.
- Push only the tag (`git push origin "$VERSION"`), not all branches — pushing everything can trigger unintended workflows.
- Tag and push **after** committing the version bump, not before.
- `cargo fetch` (not `cargo build`) is used to update `Cargo.lock` quickly.
