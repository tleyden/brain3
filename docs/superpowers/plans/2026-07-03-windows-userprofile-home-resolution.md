# Windows: resolve app home from USERPROFILE

## Problem (P1 — Windows launch fails)

On a default Windows environment, `.\brain3.exe` exits before setup with
*"HOME environment variable is not set"*. Brain3 resolves the user's home
directory from `HOME` only, but Windows normally provides `%USERPROFILE%`
(not `HOME`). Users must manually set `B3_HOME` or `HOME` first, which is a
broken first-run experience.

### Affected call sites (all in the `platform` crate)

- `crates/platform/src/setup/app_home.rs:33` — `Brain3AppHome::resolve_from_env()`
  errors and exits before setup.
- `crates/platform/src/config/env_file.rs:179` — `resolve_token_db_path()` has the
  same `HOME`-only fallback.
- `crates/platform/src/tunnel/cloudflare_setup.rs:101` — `find_credentials_file()`
  reads `HOME` directly. Lower priority (only matters once tunneling runs) but the
  same bug.

## Fix

Introduce one shared helper in the `platform` crate that resolves the user home
directory cross-platform, then route all three call sites through it.

```rust
/// Resolves the current user's home directory.
/// Prefers HOME (Unix, and respected everywhere for overrides);
/// falls back to USERPROFILE, which is the standard on Windows.
pub(crate) fn user_home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}
```

### Why try-`HOME`-then-`USERPROFILE` (not `#[cfg(windows)]`)

- Single code path — less `cfg` branching to test and maintain.
- `HOME` first preserves existing Unix behavior and honors users who deliberately
  set `HOME` on Windows (e.g. Git Bash).
- `USERPROFILE` is never set on Unix, so there is no downside to checking it there.

### Changes

1. **`app_home.rs`** — add/import the helper; in `resolve_from_env()` replace the
   `HOME` lookup with `user_home_dir()`. Update the error message to
   *"neither HOME nor USERPROFILE is set"*. `B3_HOME` override precedence stays
   first, unchanged.
2. **`env_file.rs`** — reuse the same helper in `resolve_token_db_path()`. Same
   error-message update.
3. **`cloudflare_setup.rs:101`** — swap the raw `HOME` read for the helper so
   `.cloudflared` credentials resolve on Windows too.
4. **Tests** — add a platform-crate unit test covering: `USERPROFILE`-set /
   `HOME`-unset resolves correctly, `HOME` wins when both set, and `B3_HOME` still
   overrides both. Add `"USERPROFILE"` to the `CONFIG_KEYS` cleanup list in
   `apps/gateway/src/main.rs:794` so env-mutating tests scrub it.

### Where the helper lives

Both `app_home.rs` and `env_file.rs` need it, so put it in a small
`crates/platform/src/util.rs` (or `env.rs`) as `pub(crate)` — exactly one
implementation rather than duplicating it in two modules.

## Out of scope

No new ingress, no auth/OAuth changes, no README/docs edits beyond what's needed.
Purely the home-directory resolution fix.

## Verification

- `cargo test -p brain3 --no-run` (catches `#[cfg(test)]` compile errors), then
  `cargo test`.
- Confirm platform crate tests pass. Full E2E only if requested.
