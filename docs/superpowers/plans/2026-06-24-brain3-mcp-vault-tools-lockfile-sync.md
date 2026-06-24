# Brain3 MCP Vault Tools Lockfile Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task in this session. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Regenerate `brain3-mcp-vault-tools/uv.lock` so `uv sync --locked` succeeds in the container build again, and add a fast CI check that catches future `pyproject.toml` / `uv.lock` drift before the image build step.

**Architecture:** Keep this as a packaging-consistency fix, not a container-runtime change. The container build remains unchanged and continues to enforce locked dependency installs; the fix is to resync the generated lockfile with project metadata and add a lightweight repository-level guard.

**Tech Stack:** `uv`, Python packaging metadata, GitHub Actions, Docker container smoke build, Rust workspace verification via `cargo test`

---

## File Structure

- Modify: `brain3-mcp-vault-tools/uv.lock`
  - Regenerated lockfile so the local package metadata matches `pyproject.toml`
- Modify: `.github/workflows/ci.yml`
  - Add an early lockfile consistency check on push and pull request runs
- Do not modify: `brain3-mcp-vault-tools/Containerfile`
  - `uv sync --locked` is behaving correctly and should keep failing when the lockfile is stale

### Task 1: Reproduce and confirm the stale lockfile mismatch

**Files:**
- Read: `brain3-mcp-vault-tools/pyproject.toml`
- Read: `brain3-mcp-vault-tools/uv.lock`

- [ ] **Step 1: Confirm the project version mismatch**

Run:

```bash
rg -n '^version = "0.2.2"$|^version = "0.2.1"$|^name = "brain3-mcp-vault-tools"$' \
  brain3-mcp-vault-tools/pyproject.toml \
  brain3-mcp-vault-tools/uv.lock
```

Expected:
- `brain3-mcp-vault-tools/pyproject.toml` shows `version = "0.2.2"`
- `brain3-mcp-vault-tools/uv.lock` shows the local package block still at `version = "0.2.1"`

- [ ] **Step 2: Reproduce the exact lockfile failure locally**

Run:

```bash
uv lock --project ./brain3-mcp-vault-tools --check
```

Expected: command exits non-zero with the same stale-lock message seen in CI, indicating `uv.lock` must be updated.

### Task 2: Regenerate `uv.lock` from the current project metadata

**Files:**
- Modify: `brain3-mcp-vault-tools/uv.lock`

- [ ] **Step 1: Regenerate the lockfile instead of hand-editing it**

Run:

```bash
uv lock --project ./brain3-mcp-vault-tools
```

Expected: `brain3-mcp-vault-tools/uv.lock` is rewritten by `uv`.

- [ ] **Step 2: Verify the local package metadata in the regenerated lockfile**

Check for this diff shape in `brain3-mcp-vault-tools/uv.lock`:

```toml
[[package]]
name = "brain3-mcp-vault-tools"
-version = "0.2.1"
+version = "0.2.2"
source = { editable = "." }
```

Expected: the project package entry now matches the version in `pyproject.toml`. Do not manually adjust unrelated resolved dependency entries.

- [ ] **Step 3: Verify the lockfile is now internally consistent**

Run:

```bash
uv lock --project ./brain3-mcp-vault-tools --check
```

Expected: command exits `0` with no stale-lock error.

### Task 3: Add an early CI guard for lockfile drift

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add Python setup before the Rust build-and-test steps**

Insert these steps after `actions/checkout@v4` in `.github/workflows/ci.yml`:

```yaml
      - name: Setup Python
        uses: actions/setup-python@v5
        with:
          python-version: "3.14"

      - name: Install uv
        run: python -m pip install --upgrade pip uv

      - name: Check brain3-mcp-vault-tools lockfile
        run: uv lock --project ./brain3-mcp-vault-tools --check
```

Expected: pull requests and pushes fail early with a direct lockfile error instead of failing later inside the container build.

- [ ] **Step 2: Keep the rest of `ci.yml` unchanged**

Preserve the existing Rust build, `cargo test`, and release-signing smoke test steps as-is. This change is only a preflight packaging check.

### Task 4: Verify the fix in the same places that currently fail

**Files:**
- Verify: `brain3-mcp-vault-tools/uv.lock`
- Verify: `.github/workflows/ci.yml`

- [ ] **Step 1: Run the Python project tests**

Run:

```bash
cd brain3-mcp-vault-tools
uv run python -m unittest discover -s tests -v
```

Expected: all `brain3-mcp-vault-tools` public API / startup tests pass.

- [ ] **Step 2: Re-run the container build path that was failing**

Run:

```bash
docker build -f ./brain3-mcp-vault-tools/Containerfile \
  -t brain3-mcp-vault-tools:lockfile-fix \
  ./brain3-mcp-vault-tools
```

Expected: both `uv sync --locked` layers complete successfully, and the image builds without the stale-lock error.

- [ ] **Step 3: Run the repository-wide required verification**

Run:

```bash
cargo test
```

Expected: Rust workspace tests still pass after the CI workflow update.

## Self-Review

- Spec coverage: this plan fixes the immediate `uv.lock` drift and adds a minimal prevention check in the main CI workflow.
- Placeholder scan: no TODOs or vague “fix later” steps remain.
- Type consistency: command paths and filenames match the current repository layout.

## Notes

- The root cause is metadata drift, not a broken `Containerfile`.
- The minimum viable fix is regenerating `brain3-mcp-vault-tools/uv.lock`.
- The recommended fix is regenerating the lockfile plus adding the `uv lock --check` CI gate so version bumps cannot silently break `uv sync --locked` again.
