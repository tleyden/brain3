# P1: Windows cloudflared detection fails (`which` not available)

## Problem

`check_cloudflared_installed()` and two copies of `cloudflared_on_path()` shell out to the
Unix `which` command to detect whether `cloudflared` is on `PATH`. On a native Windows
`brain3.exe` run there is no `which` binary, so the spawn fails and detection returns
`false` even when `cloudflared.exe` is correctly installed and on `PATH`. This makes the
new Windows build's setup TUI report *"cloudflared: not found"* incorrectly.

Affected code (three duplicated implementations, all identical logic):

- `crates/platform/src/tunnel/cloudflare_setup.rs:8` — `check_cloudflared_installed()` (async, pub)
- `crates/platform/src/tunnel/cloudflare_quick.rs:31` — `cloudflared_on_path()` (sync, private)
- `crates/platform/src/tunnel/cloudflare_named.rs:46` — `cloudflared_on_path()` (sync, private)

The `setup_tui` caller at `apps/gateway/src/setup_tui.rs:135` consumes
`check_cloudflared_installed()` and drives the "cloudflared: found / not found" step.

## Research: cross-platform executable lookup in Rust

Two mature crates resolve an executable through `PATH` in a platform-correct way,
including honoring Windows `PATHEXT` (so `cloudflared` resolves to `cloudflared.exe`):

- **`which`** (v8.0.4, June 2026) — the de-facto standard, a Rust equivalent of Unix
  `which(1)`, used by cargo/rustup. `which::which("cloudflared") -> Result<PathBuf>`.
  Handles Windows `PATHEXT` resolution out of the box. Small dependency footprint.
- **`pathsearch`** — `find_executable_in_path("cloudflared") -> Option<PathBuf>`, also
  honors `PATHEXT` on Windows. Less widely adopted.

**Recommendation: use the `which` crate.** It is the ecosystem standard, is the exact
`which(1)` replacement we want, and drops us cleanly into a `Result`/`Option` check with
no subprocess spawn at all (pure filesystem lookup — also faster and avoids the process
overhead we currently pay on every check).

Ecosystem confidence for this choice is high, and deliberately so for an infrastructure
project: `which` is used throughout the Rust ecosystem and appears in countless projects
and examples, so the code is immediately recognizable to any Rust developer. `pathsearch`
is technically sound but a niche utility crate — a survey turned up essentially no
Reddit/Stack Overflow discussion comparing the two, which itself signals that developers
overwhelmingly reach for `which` without debating alternatives. There is no active
tradeoff controversy to weigh. For Brain3 we optimize for **ecosystem convention over
shaving one tiny dependency**; the maintenance risk of adopting `which` is far lower than
relying on shelled-out `which`/`where` commands or a less-established crate.

## Plan

### 1. Add the dependency

In `crates/platform/Cargo.toml` add:

```toml
which = "8"
```

### 2. Add one shared helper, remove the three duplicates

There are currently three copies of the same logic. Consolidate into a single
`pub(crate)` helper so the fix lives in one place. Return the resolved `PathBuf` rather
than a bare `bool` — `which` gives us the path for free, and having it available means we
can log *where* `cloudflared` was found (useful diagnostics on Windows PATH issues). Put it
next to the existing `user_home_dir()` in `crates/platform/src/util.rs`:

```rust
use std::path::PathBuf;

/// Resolves the `cloudflared` executable on PATH, returning its absolute path.
/// Cross-platform: on Windows this honors PATHEXT (resolves `cloudflared.exe`),
/// so it works on a native brain3.exe run where the Unix `which` command is absent.
pub(crate) fn find_cloudflared() -> Option<PathBuf> {
    which::which("cloudflared").ok()
}
```

Then:

- **`cloudflare_quick.rs`** — delete the local `cloudflared_on_path()` (lines 31–37); at
  the existing call site (line 42) use `crate::util::find_cloudflared().is_none()` for the
  not-found guard.
- **`cloudflare_named.rs`** — delete the local `cloudflared_on_path()` (lines 46–52),
  update its call site the same way.
- **`cloudflare_setup.rs`** — reimplement `check_cloudflared_installed()` to delegate:

  ```rust
  pub async fn check_cloudflared_installed() -> bool {
      crate::util::find_cloudflared().is_some()
  }
  ```

  Keep the `async` signature so the `setup_tui.rs:135` caller (`.await`) is unchanged.
  The lookup is a handful of filesystem `stat`s — cheap and non-blocking enough to call
  directly. If we want to be strict about "no blocking I/O in async" (per AGENTS.MD), wrap
  it in `tokio::task::spawn_blocking(|| crate::util::find_cloudflared()).await`; noted
  as optional, likely overkill here.

  Optional diagnostics win: since `find_cloudflared()` now returns the path, we can
  `tracing::debug!(path = %p.display(), "cloudflared resolved")` in the found branch, which
  makes Windows PATH misconfigurations far easier to debug from logs.

Net result: the `which` string literal and `std::process::Command::new("which")` calls are
gone from the tunnel module entirely.

### 3. Regression test (the repro harness, promoted to a check-in test)

Add a focused behavioral test that exercises **our helper**, not the third-party crate's
internals. It simulates the Windows-relevant case: an executable present in a directory on
`PATH`, discoverable only via extension resolution.

- Create a `tempfile::TempDir`, write a fake `cloudflared` (and, to mirror Windows,
  `cloudflared.exe`) marked executable on Unix.
- Use `which::which_in("cloudflared", Some(path), cwd)` semantics inside a small wrapper
  the test can call, OR set `PATH` to the temp dir for the duration of the test and assert
  `find_cloudflared()` is `Some`, then assert `None` for a name that does not exist.

Note: `std::env::set_var` mutating global `PATH` is process-wide; guard the test so it does
not race other tests (single test, or serialize). Keep this to **one** test focused on the
found/not-found behavior of our helper — do not test the `which` crate's PATHEXT parsing
itself (that is the dependency's responsibility and would be brittle per our testing rules).

If a reliable cross-platform env-mutating test proves too flaky, fall back to documenting a
manual Windows verification step (install `cloudflared.exe` on PATH, run `brain3.exe` setup,
confirm "cloudflared: found") and skip the automated test rather than add a flaky one.

### 4. Verify locally

- `cargo test -p brain3-platform --no-run` (compile test targets)
- `cargo test -p brain3-platform` (run unit tests, incl. new regression test)
- `cargo build` for the workspace to confirm the new dep resolves.
- Manual/host sanity: on macOS, `check_cloudflared_installed()` still returns `true` when
  `cloudflared` is installed and `false` when it is not (unchanged behavior).

## Out of scope / notes

- No new points of ingress, no auth surface touched — no SECURITY_AUDIT.MD threat-model
  change required.
- The other `cloudflared` subprocess invocations (`tunnel login`, `tunnel list`, etc.) are
  unaffected: those correctly invoke `cloudflared` directly, and the OS resolves the `.exe`
  on Windows. Only the `which`-based *detection* was broken.
- `README.md`/version files are not touched — this is a bug fix, not a release bump.

## Sources

- [`which` crate — docs.rs](https://docs.rs/which/latest/which/)
- [`pathsearch` crate — docs.rs](https://docs.rs/pathsearch/latest/pathsearch/)
- [rust-lang/cargo #10455 — recognize $PATHEXT on Windows](https://github.com/rust-lang/cargo/issues/10455)
