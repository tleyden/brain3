# E2E Fail-Fast CI RCA Fix Implementation Plan

> **For agentic workers:** Use `superpowers:executing-plans` to implement this plan task-by-task. Do not use subagents; this repo's `AGENTS.md` says work should stay serial.

**Goal:** Fix the CI e2e failure caused by passing `--fail-fast` to libtest on the stable Rust toolchain, while preserving local-first, then OAuth, stop-on-first-failure behavior.

**Architecture:** Remove `--fail-fast` from libtest arguments. The script will run the e2e tests as separate filtered cargo invocations in the desired order for default CI runs, and normal process exit codes will stop the run after the first failed cargo command. Direct extra-arg/debug runs keep using a single cargo invocation with caller-supplied filters.

**Tech Stack:** Python `unittest`, Rust libtest, Cargo integration tests, GitHub Actions stable Rust.

---

## RCA

### What Failed

CI failed in `.github/workflows/e2e.yml` during:

```bash
uv run scripts/e2e_smoke.py
```

The script built the Docker image, compiled the e2e test target, then invoked the Rust test harness with:

```bash
cargo test -p brain3 --test e2e_smoke --features e2e -- --nocapture --test-threads=1 --fail-fast
```

CI's stable Rust libtest rejected `--fail-fast`:

```text
error: The "fail-fast" flag is only accepted on the nightly compiler with -Z unstable-options
```

### Root Cause

`scripts/e2e_smoke.py` unconditionally passes `--fail-fast`, but that flag is not stable across the Rust toolchain used by CI.

Local verification was not representative:

```bash
rustc --version
# rustc 1.96.0 (ac68faa20 2026-05-25)
```

On that local toolchain, `cargo test -p brain3 --test e2e_smoke --features e2e -- --help` advertises `--fail-fast` as accepted. CI uses `dtolnay/rust-toolchain@stable`; its stable libtest still treats `--fail-fast` as nightly-only.

### Contributing Cause

The previous local validation checked whether the local harness accepted `--fail-fast`, but did not test against the same stable toolchain behavior CI uses. The e2e script itself could not be fully run locally because Docker was unavailable, so the failing CI command path was not exercised end-to-end.

### Fix Strategy

Do not pass `--fail-fast` to libtest.

`--fail-fast` only means "stop starting more tests after a failure." We do not need that harness flag. We can remove it and get the same intended e2e behavior by running one test command at a time and returning immediately if any command fails.

Preserve the intended behavior in Python:

1. Build the Docker image once.
2. Run the local e2e test by exact filter.
3. If it fails, return immediately.
4. Run the OAuth public-flow e2e test by exact filter.
5. Return that exit code.

This keeps ordering and fail-fast behavior deterministic without depending on unstable libtest flags.

## Files

- Modify: `scripts/e2e_smoke.py`
- Modify: `scripts/test_e2e_smoke.py`
- No Rust source changes expected.
- No docs outside this plan.
- No git commit; user will commit.

## Task 1: Add Script Tests For Portable Ordered Fail-Fast

**Files:**
- Modify: `scripts/test_e2e_smoke.py`

- [x] **Step 1: Update the existing explicit-filter test to remove `--fail-fast`**

Change the expected command in `test_runs_build_before_cargo_and_forwards_extra_args` so it still expects `--test-threads=1`, but no longer expects `--fail-fast`:

```python
[
    "cargo",
    "test",
    "-p",
    "brain3",
    "--test",
    "e2e_smoke",
    "--features",
    "e2e",
    "--",
    "--nocapture",
    "--test-threads=1",
    "e2e_smoke_starts_gateway",
]
```

- [x] **Step 2: Add a failing test for default ordered execution**

Add this test to `E2ESmokeScriptTests`:

```python
def test_default_run_executes_local_then_oauth_tests(self):
    module = load_script()
    calls = []

    def fake_run(command, cwd):
        calls.append((command, cwd))
        return 0

    exit_code = module.run([], run_command=fake_run)

    self.assertEqual(exit_code, 0)
    self.assertEqual(len(calls), 3)
    self.assertEqual(calls[0][0][0:2], ["docker", "build"])
    self.assertEqual(calls[1][0][-1], "e2e_smoke_1_local_docker")
    self.assertEqual(calls[2][0][-1], "e2e_smoke_2_oauth_public_flow")
    self.assertNotIn("--fail-fast", calls[1][0])
    self.assertNotIn("--fail-fast", calls[2][0])
```

- [x] **Step 3: Add a failing test that OAuth is skipped if local fails**

Add this test to `E2ESmokeScriptTests`:

```python
def test_default_run_aborts_before_oauth_when_local_test_fails(self):
    module = load_script()
    calls = []

    def fake_run(command, cwd):
        calls.append((command, cwd))
        if command[-1] == "e2e_smoke_1_local_docker":
            return 23
        return 0

    exit_code = module.run([], run_command=fake_run)

    self.assertEqual(exit_code, 23)
    self.assertEqual(len(calls), 2)
    self.assertEqual(calls[0][0][0:2], ["docker", "build"])
    self.assertEqual(calls[1][0][-1], "e2e_smoke_1_local_docker")
```

- [x] **Step 4: Run the script tests and verify they fail for the intended reason**

Run:

```bash
python3 -m unittest scripts.test_e2e_smoke -v
```

Expected:

```text
FAIL: test_default_run_executes_local_then_oauth_tests
FAIL: test_default_run_aborts_before_oauth_when_local_test_fails
```

The existing explicit-filter test may also fail until Task 2 removes `--fail-fast`.

## Task 2: Implement Portable Ordered Execution

**Files:**
- Modify: `scripts/e2e_smoke.py`

- [x] **Step 1: Add named default e2e filters**

Near `IMAGE_TAG`, add:

```python
DEFAULT_E2E_TESTS = [
    "e2e_smoke_1_local_docker",
    "e2e_smoke_2_oauth_public_flow",
]
```

- [x] **Step 2: Remove `--fail-fast` from `cargo_test_command`**

Change `cargo_test_command` to:

```python
def cargo_test_command(extra_args: Sequence[str]) -> list[str]:
    return [
        "cargo",
        "test",
        "-p",
        "brain3",
        "--test",
        "e2e_smoke",
        "--features",
        "e2e",
        "--",
        "--nocapture",
        "--test-threads=1",
        *extra_args,
    ]
```

- [x] **Step 3: Sequence default test execution in `run`**

Replace the final line of `run`:

```python
return run_command(cargo_test_command(extra_args), root)
```

with:

```python
    if extra_args:
        return run_command(cargo_test_command(extra_args), root)

    for test_name in DEFAULT_E2E_TESTS:
        test_exit_code = run_command(cargo_test_command([test_name]), root)
        if test_exit_code != 0:
            return test_exit_code

    return 0
```

- [x] **Step 4: Run the script tests and verify they pass**

Run:

```bash
python3 -m unittest scripts.test_e2e_smoke -v
```

Expected:

```text
Ran 4 tests
OK
```

## Task 3: Verify Rust And E2E Command Compatibility

**Files:**
- No edits expected.

- [x] **Step 1: Compile the e2e target**

Run:

```bash
cargo test -p brain3 --test e2e_smoke --features e2e --no-run
```

Expected: exit code 0.

- [x] **Step 2: Verify the generated default cargo command does not contain `--fail-fast`**

Run:

```bash
python3 - <<'PY'
import importlib.util
from pathlib import Path

path = Path("scripts/e2e_smoke.py")
spec = importlib.util.spec_from_file_location("e2e_smoke", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

command = module.cargo_test_command(["e2e_smoke_1_local_docker"])
print(command)
assert "--test-threads=1" in command
assert "--fail-fast" not in command
assert command[-1] == "e2e_smoke_1_local_docker"
PY
```

Expected: exit code 0 and printed command has no `--fail-fast`.

- [x] **Step 3: Run standard required Rust verification**

Run:

```bash
cargo test -p brain3 --no-run
cargo test
```

Expected: both exit code 0.

- [ ] **Step 4: Run full e2e if Docker is available**

Run:

```bash
uv run scripts/e2e_smoke.py
```

Expected when Docker is available:

```text
test e2e_smoke_1_local_docker ... ok
test e2e_smoke_2_oauth_public_flow ... ok
```

If Docker is unavailable locally, record the exact Docker daemon error and rely on CI to run the Docker-backed e2e after the script-level command issue is fixed.

Local result on 2026-07-03: Docker was unavailable before cargo started:

```text
ERROR: failed to connect to the docker API at unix:///Users/tleyden/.docker/run/docker.sock; check if the path is correct and if the daemon is running: dial unix /Users/tleyden/.docker/run/docker.sock: connect: no such file or directory
Docker image build failed with exit code 1; aborting before running the E2E smoke test.
```

## Self-Review Checklist

- [x] The plan removes the CI-incompatible `--fail-fast` flag.
- [x] The default script path still runs local before OAuth.
- [x] The default script path still stops before OAuth if local fails.
- [x] Explicit/debug `extra_args` remain supported.
- [x] No OAuth server behavior or security policy changes.
- [x] No new ingress, no threat model update required.
- [x] No git commit.
