# Plan: Persistent log file in brain3 home

## Goal

Stop creating a new temp log file on every startup. Instead write to a stable,
persistent path under `--brain3-home` so logs survive restarts and are easy to find.

**Default path:** `~/.brain3/brain3.log` (or `$B3_HOME/brain3.log` if overridden).  
**Extension/name convention:** `.log`, single file, append-only. No rotation needed yet.

---

## Files touched

| File | Change |
|------|--------|
| `crates/core/src/ports/setup_system.rs` | Rename `create_temp_log_file` → `resolve_log_file`; change it to accept `&SetupPaths` so the impl knows where `app_home` is |
| `crates/platform/src/setup/system.rs` | Implement `resolve_log_file`: return `paths.app_home.join("brain3.log")`, create parent dir if absent |
| `apps/gateway/src/logging.rs` | Call `resolve_log_file` instead of `create_temp_log_file`; pass resolved paths; open in append mode (already the case) |
| `crates/core/src/application/first_run_setup.rs` | Update mock impl of `create_temp_log_file` → `resolve_log_file`; keep returning `/tmp/brain3.log` for tests |
| `crates/platform/src/runtime/bootstrap.rs` | Update test fixture hardcoded `/tmp/brain3.log` references (no logic change) |
| `apps/gateway/src/tui/app.rs` | Same — test fixture `/tmp/brain3.log` references |
| `apps/gateway/src/tui/screens.rs` | Same — test fixture `/tmp/brain3.log` references |

---

## Step-by-step

### 1. Update port trait (`setup_system.rs`)

```rust
// Before
async fn create_temp_log_file(&self) -> Result<PathBuf, SetupError>;

// After
async fn resolve_log_file(&self, paths: &SetupPaths) -> Result<PathBuf, SetupError>;
```

The impl ensures the app-home directory structure exists via `ensure_app_home_dirs`
(best-effort, don't fail startup if this fails — fall back to temp file and log a
warning).

### 2. Platform implementation (`system.rs`)

```rust
async fn resolve_log_file(&self, paths: &SetupPaths) -> Result<PathBuf, SetupError> {
    // Create app-home dirs if they don't exist yet (first run before setup wizard).
    if let Err(e) = self.ensure_app_home_dirs(paths).await {
        tracing::warn!(path = %paths.app_home.display(), error = %e,
            "could not create app home dirs for log file, falling back to temp dir");
        // fall back to old behaviour
        let path = env::temp_dir().join("brain3.log");
        return Ok(path);
    }
    Ok(paths.app_home.join("brain3.log"))
}
```

### 3. Gateway logging init (`logging.rs`)

`init_logging` currently creates a `PlatformSetupSystem` and calls `create_temp_log_file`.
Change it to also call `setup_system.resolve_paths()` and pass paths through:

```rust
pub async fn init_logging(default_level: &str) -> Result<GatewayLogging> {
    let setup_system = PlatformSetupSystem::new();
    let paths = setup_system.resolve_paths()
        .unwrap_or_else(|_| SetupPaths::new(
            env::temp_dir().join("brain3"),
            env::temp_dir().join("brain3/.env"),
            env::temp_dir().join("brain3/cloudflared"),
        ));
    let log_file = setup_system
        .resolve_log_file(&paths)
        .await
        .unwrap_or_else(|_| env::temp_dir().join("brain3.log"));
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&log_file)?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    // rest unchanged
}
```

If `resolve_paths` fails (no HOME set, unusual), we fall back gracefully without crashing.
The logging initializer must create the file on first run and enforce `0600`
permissions on Unix so persistent logs are not left world-readable by umask or
pre-existing file mode.

### 4. Update mock and test fixtures

- `first_run_setup.rs` mock: rename method, keep returning `/tmp/brain3.log`
- `bootstrap.rs`, `app.rs`, `screens.rs`: update the 5–6 hardcoded `/tmp/brain3.log` strings (cosmetic, no logic change)

---

## Cloudflare YAML audit trail

Brain3 **writes** the YAML once (during setup wizard) and **reads** it on every
startup for port validation. It **never deletes** it — the only `remove_file` call
in the codebase (`lifecycle.rs:23`) removes the PID file, not the YAML.

Currently none of the write/read operations emit tracing events. Add them:

| File | Location | What to log |
|------|----------|-------------|
| `crates/platform/src/tunnel/cloudflare_setup.rs` | `write_config_file`, before `fs::write` | `tracing::info!` with `config_path`, `tunnel_id`, `tunnel_name`, `domain`, `local_port` — e.g. `"writing cloudflare tunnel config"` |
| `crates/platform/src/tunnel/cloudflare_setup.rs` | `write_config_file`, after successful `fs::write` | `tracing::info!` confirming write with `config_path` — e.g. `"cloudflare tunnel config written"` |
| `crates/platform/src/tunnel/startup.rs` | `validate_named_tunnel_config_port`, before `fs::read_to_string` | `tracing::info!` with `config_file` — e.g. `"reading cloudflare tunnel config for port validation"` |
| `crates/platform/src/tunnel/startup.rs` | `validate_named_tunnel_config_port`, after successful `read_to_string` | `tracing::info!` confirming read — e.g. `"cloudflare tunnel config read successfully"` with `config_file` and byte length |
| `crates/platform/src/tunnel/cloudflare_named.rs` | `start()`, after `config_file.exists()` returns true (line 111) | `tracing::info!` confirming file found — e.g. `"cloudflare tunnel config file present"` with `config_file` |
| `apps/gateway/src/setup_tui.rs` | step 4 write block (line 459) | Add `tracing::info!` alongside the existing TUI state log — same path info |

### Log lines to add (representative)

```rust
// cloudflare_setup.rs::write_config_file — before write
tracing::info!(
    config_path = %config_path.display(),
    tunnel_id = %tunnel_id,
    tunnel_name = %tunnel_name,
    domain = %domain,
    local_port,
    "writing cloudflare tunnel config"
);

// cloudflare_setup.rs::write_config_file — after write
tracing::info!(config_path = %config_path.display(), "cloudflare tunnel config written");

// startup.rs::validate_named_tunnel_config_port — before read
tracing::info!(config_file = %config_file.display(), "reading cloudflare tunnel config for port validation");

// startup.rs::validate_named_tunnel_config_port — after successful read
tracing::info!(config_file = %config_file.display(), bytes = content.len(), "cloudflare tunnel config read successfully");

// cloudflare_named.rs::start — config file present check
tracing::info!(config_file = %self.config_file.display(), "cloudflare tunnel config file present");
```

Note: no deletion logging is needed because brain3 never deletes the YAML.

### How `.env` ends up with the path but the YAML doesn't exist

The `.env` write (`finalize()`) and the YAML write (`write_config_file`) are **completely separate
steps** with no transaction between them. Four distinct scenarios produce the observed state:

---

**Scenario 1 — External deletion (most likely for the user's case)**

1. First-run wizard → `finalize()` writes `.env` with `B3_CF_TUNNEL_CONFIG_FILE=…brain3-dev.yml`
2. `setup_tui::run()` completes → `write_config_file` writes the YAML → tunnel runs fine
3. Something external (accidental `rm`, system cleanup, disk operation) deletes the YAML
4. Next `brain3` start: `named_tunnel_setup_config()` sees `!config_file.exists()` → re-triggers `setup_tui`

Brain3 is NOT the cause — confirmed by code audit: the only `remove_file` call in the codebase
removes `cloudflared.pid`, not the YAML.

**Audit trail gap:** no tracing log is emitted when `write_config_file` is called, so there's no
record in the log file of _when_ the YAML was last written. Fix: add the `tracing::info!` calls
described above.

---

**Scenario 2 — `setup_tui` abandoned before step 4**

1. Brain3 detects YAML missing → launches `setup_tui`
2. Steps 0-3 succeed (cloudflared installed, logged in, tunnel found, credentials found)
3. User quits the TUI or a non-fatal error occurs before step 4 (`write_config_file`)
4. `.env` unchanged with the path, YAML still doesn't exist

Next run re-triggers `setup_tui`. No data loss. But without logging there's no record that step 4
was ever reached or skipped.

---

**Scenario 3 — First-run wizard wrote `.env`, YAML never provisioned**

Code path (`app.rs::finalize_and_start`):

```
finalize()          → writes .env with CF path     [env_writer.rs:144]
                      (B3_CF_TUNNEL_CONFIG_FILE set, YAML does NOT exist yet)
start_configured_runtime_session()
  → bootstrap_configured_runtime()
    → ensure_named_tunnel_config_exists()           [bootstrap.rs:366]
      → config_file.exists() == false
      → logs error + bails
```

The first-run wizard does NOT call `setup_tui` or `write_config_file`. The YAML is only written
when `setup_tui::run()` is called — either explicitly via `--cf-setup` or automatically on the
NEXT `brain3` start (when `named_tunnel_setup_config()` returns `Some`).

So after a fresh first-run wizard with a named tunnel: `.env` has the path, YAML doesn't exist
yet, brain3 exits with an error. This is expected — the next run triggers `setup_tui`.

---

**Scenario 4 — `B3_HOME` changed between runs**

1. Brain3 ran with `B3_HOME=/path/A`, YAML written to `/path/A/cloudflared/brain3-dev.yml`
2. `B3_HOME` changed to `/path/B` (or unset, falling back to `~/.brain3`)
3. New `.env` at `/path/B/.env` has `B3_CF_TUNNEL_CONFIG_FILE=/path/B/cloudflared/brain3-dev.yml`
4. YAML still lives at `/path/A/cloudflared/brain3-dev.yml` — wrong path

The user's case rules this out (`.env` and YAML path share the same `.brain3_dev` root), but it's
a realistic source of confusion when running with different `B3_HOME` overrides.

**Audit trail gap:** no log at startup showing _which_ `B3_HOME` / `B3_CF_TUNNEL_CONFIG_FILE` is
active. Fix: `env_file.rs:485` already logs `config_file`, and `main.rs:715-718` logs
`config_file_exists`; those cover this case if logs are persistent.

---

## What does NOT change

- Log format, log level filtering, `RUST_LOG` env var — all unchanged
- The TUI "Logs" field already displays `launch_plan.log_file` — it will now just show the persistent path
- `tracing_appender::non_blocking` already opens in append mode — no change needed

---

## Out of scope

- Log rotation (can add `tracing-appender` rolling appender later)
- Surfacing old log location during migration (there's no migration; each run picks up the new path)
