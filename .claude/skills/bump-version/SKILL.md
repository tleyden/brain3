---
name: bump-version
description: Bump the release version everywhere in the Brain3 project — Cargo.toml, pyproject.toml, README, first_run_setup.rs, and test fixtures — then optionally tag and push.
---

# bump-version

Updates all six version references in the repo atomically, then interactively asks whether to tag and push.

## Files updated

| File | What changes |
|------|-------------|
| `apps/gateway/Cargo.toml` | `version = "X.Y.Z"` |
| `brain3-mcp-vault-tools/pyproject.toml` | `version = "X.Y.Z"` |
| `README.MD` | S3 install URL (`/releases/vX.Y.Z/install.sh`) |
| `crates/core/src/application/first_run_setup.rs` | `CURRENT_RELEASE = "vX.Y.Z"` |
| `brain3-mcp-vault-tools/tests/test_server_startup.py` | Two version string fixtures |
| `Cargo.lock` | Auto-updated via `cargo fetch` |

## Usage

```bash
bash .claude/skills/bump-version/bump.sh 0.2.3
```

The script:
1. Detects the current version from `apps/gateway/Cargo.toml`
2. Applies sed replacements to all six locations
3. Runs `cargo fetch` to refresh `Cargo.lock`
4. Prints the commit command to use
5. **Asks you** whether to tag and push — answers `y` to proceed, anything else to skip

## Tag and push (prompted)

After the file edits print, the script asks:

```
Tag and push v0.2.3 now? [y/N]
```

Answer `y` to run:
```bash
git tag -a "v0.2.3" -m "Release v0.2.3"
git push
git push --tags
```

Answer `n` (or press Enter) to skip. The script prints the manual commands so you can run them after reviewing the commit.

## Gotchas

- The version argument can have or omit a leading `v` — both `0.2.3` and `v0.2.3` work.
- `cargo fetch` is used (not `cargo build`) to keep the Cargo.lock update fast.
- The tag-and-push step does **not** commit — commit first, then re-run or answer `y` when prompted.
