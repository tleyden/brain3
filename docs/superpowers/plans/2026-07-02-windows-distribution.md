# Windows Distribution

## Goal

Produce a Windows `brain3.exe` build artifact from the GitHub release automation, alongside the existing Linux/macOS targets.

## Reality check: it is NOT "just cross-compile"

Adding a Windows row to the release matrix is easy. But the crate does **not compile on Windows today**, and even once it compiles there are runtime gaps. This plan splits the work into: (A) make it build, (B) ship the artifact, (C) optional functional hardening.

### What blocks a Windows build

| Location | Issue | Severity | Fix |
| --- | --- | --- | --- |
| `crates/platform/src/container/startup.rs:139` | `libc::getuid()` / `libc::getgid()` called **ungated**. `libc` has no `getuid`/`getgid` on Windows → hard compile error. | Compile blocker | `cfg`-gate into an `Option<String>` `user` (see Step 1). Clean — consumers already treat `user` as optional. |
| `crates/platform/Cargo.toml:9` | `libc` is an unconditional dep; fine to keep, but Windows code paths must not reference unix-only symbols. | Minor | Keep as-is; links fine once `startup.rs` is gated. Optional later cleanup: move under `[target.'cfg(unix)'.dependencies]`. |

### Already handled (no action needed)

- `tunnel/lifecycle.rs` `libc::kill` → already `#[cfg(unix)]`.
- `tunnel/cloudflare_quick.rs` / `cloudflare_named.rs` `prctl(PR_SET_PDEATHSIG)` → already `#[cfg(target_os = "linux")]`.
- `logging.rs` / `diagnostics.rs` signal handling → already `#[cfg(unix)]` with `#[cfg(not(unix))]` fallbacks.

### Runtime gaps (work, but not correctly, on Windows)

- `setup/app_home.rs:33` requires `HOME`; Windows sets `USERPROFILE`. `B3_HOME` override still works, but default resolution fails.
- `setup/system.rs` cloudflared install uses apt (linux). Windows has no auto-install path → user must install cloudflared manually (winget/choco) and have it on PATH. The existing `cloudflared_on_path()` check already degrades gracefully.
- uid/gid `--user` mapping is meaningless on Docker Desktop for Windows.

## Scope decision (needs your call)

**Option 1 — Artifact only (recommended first step).** Make it compile + emit the `.exe`, so a Windows binary exists and Docker-based workflows work. Accept that first-run cloudflared setup is manual on Windows. Smallest, shippable.

**Option 2 — Full functional parity.** Also fix `HOME`→`USERPROFILE`, add a cloudflared install/guidance path for Windows, and audit container `--user` semantics. Larger, more testing.

This plan assumes **Option 1** and lists Option-2 items as follow-ups.

## Plan (Option 1)

### Step 1 — Make `crates/platform` compile on Windows

**How to deal with the `libc::getuid()`/`getgid()` blocker (`container/startup.rs:139`).**

Good news: the plumbing already supports the clean fix. `ContainerConfig.user` is already `Option<String>`, and every consumer already omits the flag when it's `None`:
- `docker.rs:230` — `if let Some(ref user) = config.user { args.push("--user".into()); ... }`
- `macos_container.rs:320` — same pattern.

So the entire fix is to make `uid_gid` an `Option<String>` that is `Some` only on unix, and pass it straight through. No downstream changes, and it's semantically correct — Docker Desktop on Windows does not use unix uid/gid `--user` mapping.

Replace the ungated block at `startup.rs:139`:

```rust
// before
let uid_gid = format!("{}:{}", unsafe { libc::getuid() }, unsafe { libc::getgid() });
// ...
user: Some(uid_gid),
```

```rust
// after
#[cfg(unix)]
let user = Some(format!("{}:{}", unsafe { libc::getuid() }, unsafe { libc::getgid() }));
#[cfg(not(unix))]
let user: Option<String> = None; // Docker Desktop on Windows doesn't need --user
// ...
user,
```

(Field-init shorthand `user,` replaces `user: Some(uid_gid)`.)

- Search for any other ungated `libc::` / `std::os::unix` usage in non-test code and gate it — grep confirms `startup.rs:139` is the **only** currently-ungated one; the rest are already `#[cfg(unix)]` / `#[cfg(target_os = "linux")]`.
- `libc` stays a normal (unconditional) dependency in `platform/Cargo.toml`. On Windows the crate still links fine because no unix-only symbols are referenced once the above is gated. (Optionally move it under `[target.'cfg(unix)'.dependencies]` later, but that's not required and is a separate cleanup.)

### Step 2 — Verify it compiles for Windows locally (best-effort)

Native Windows isn't available on this macOS box, so verify via the check-only cross target:

```bash
rustup target add x86_64-pc-windows-gnu
cargo check -p brain3 --target x86_64-pc-windows-gnu   # catches cfg-gating mistakes
```

(This is a smoke check; the authoritative build is the CI `windows-latest` runner using the MSVC toolchain.)

### Step 3 — Add Windows to `.github/workflows/release.yml`

Add a matrix row using a native Windows runner (mirrors how macOS/Linux build natively — simpler than cross-compiling from Linux):

```yaml
- target: x86_64-pc-windows-msvc
  os: windows-latest
  display: Windows x86_64
```

Packaging needs a Windows-aware step because:
- the binary is `brain3.exe`, and
- the `Package`/`Validate tag`/manifest steps are bash. `windows-latest` provides Git Bash, and `actions/*` steps run cross-platform, but `run:` defaults to PowerShell on Windows.

Approach (keep the signing/manifest pipeline unchanged — it globs `brain3-*.tar.gz`):
- Set `defaults.run.shell: bash` at the job level so the existing bash steps (`awk` tag validation, `tar`) work on `windows-latest` too. `tar` and `awk` ship with Git Bash on the runner.
- Package as `.tar.gz` for consistency: `tar -czf brain3-<target>.tar.gz -C target/<target>/release brain3.exe`. Keeping `.tar.gz` (not `.zip`) means **zero changes** to `generate-release-manifest.sh`, `upload-to-s3.sh`, and the verify step, all of which match `brain3-*.tar.gz`.

### Step 4 — Add Windows target to `scripts/upload-to-s3.sh`

Append `"x86_64-pc-windows-msvc"` to the `TARGETS` array so the S3 publish loop uploads it. (It skips-with-warning on missing tarballs, so this is safe.)

### Step 5 — README

Add the Windows download/target to the install/download section in `README.md` per the release-update file list in AGENTS.MD. Note the manual cloudflared prerequisite on Windows (Option 1 caveat).

### Step 6 — CI sanity

- Confirm `ci.yml` doesn't need a Windows job (out of scope; release-only ask). Leave `ci.yml` as-is unless you want Windows in PR CI.
- The release workflow only runs on `v*` tags, so this won't affect normal PRs.

## Explicitly out of scope (Option 2 follow-ups)

- `HOME`→`USERPROFILE` fallback in `app_home.rs`.
- Windows cloudflared install/guidance (winget/choco) in first-run setup.
- Container `--user` semantics audit for Docker Desktop on Windows.
- Windows in PR CI (`ci.yml`) and e2e smoke on Windows.
- Threat-model update: none required — no new ingress; same binary, new build target. (Confirm during review.)

## Verification

- Local: `cargo check -p brain3 --target x86_64-pc-windows-gnu` passes; `cargo test` (host) still passes.
- CI: release workflow's new `windows-latest` job builds, packages `brain3-x86_64-pc-windows-msvc.tar.gz`, and it flows through the existing manifest/sign/verify/S3 steps unchanged.

## Files touched (Option 1)

- `crates/platform/src/container/startup.rs` — cfg-gate uid/gid.
- `.github/workflows/release.yml` — Windows matrix row + bash default shell + `.exe` packaging.
- `scripts/upload-to-s3.sh` — add Windows target.
- `README.md` — Windows download entry + caveat.
