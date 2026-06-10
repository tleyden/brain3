# Release Process (AI Generated)

## Prerequisites

- `gh` CLI installed and authenticated (`gh auth login`)
- You are on the `main` branch with a clean working tree
- All milestone issues for the release are closed

## 1. Prepare release notes from a GitHub milestone

Create a milestone in advance (e.g. `v0.2.0`) and assign issues and PRs to it as you work.

When ready to release, generate a notes draft from the closed milestone issues:

```bash
VERSION=v0.2.0

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
VERSION=v0.2.0

git tag -a "$VERSION" -m "Release $VERSION"
git push origin "$VERSION"
```

Pushing the tag triggers `.github/workflows/release.yml`, which:

1. Builds `brain3-gateway` for all four targets in parallel:
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu`
   - `x86_64-apple-darwin`
   - `aarch64-apple-darwin`
2. Creates a GitHub Release and attaches a `.tar.gz` for each target.

The workflow takes ~5–10 minutes. Monitor it:

```bash
gh run watch
```

## 3. Attach your release notes

Once the workflow completes, edit the release to replace the auto-generated notes with your draft:

```bash
VERSION=v0.2.0

gh release edit "$VERSION" \
  --notes-file /tmp/release-notes.md
```

Or edit in the browser:

```bash
gh release view "$VERSION" --web
```

## 4. Verify the release

```bash
VERSION=v0.2.0

# List attached assets
gh release view "$VERSION"

# Download and smoke-test the Linux binary
gh release download "$VERSION" \
  --pattern "brain3-gateway-x86_64-unknown-linux-gnu.tar.gz" \
  --dir /tmp/brain3-test

tar -xzf /tmp/brain3-test/brain3-gateway-x86_64-unknown-linux-gnu.tar.gz -C /tmp/brain3-test
/tmp/brain3-test/brain3-gateway --help
```

## Versioning

Follow [Semantic Versioning](https://semver.org):

- **Patch** (`v0.1.1`): bug fixes, no behaviour changes
- **Minor** (`v0.2.0`): new features, backwards-compatible
- **Major** (`v1.0.0`): breaking changes to config or API surface
