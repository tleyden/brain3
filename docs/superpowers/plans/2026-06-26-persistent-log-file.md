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

The impl creates `paths.app_home` dir if missing (best-effort, don't fail startup if
this fails — fall back to temp file and log a warning).

### 2. Platform implementation (`system.rs`)

```rust
async fn resolve_log_file(&self, paths: &SetupPaths) -> Result<PathBuf, SetupError> {
    // Create app_home if it doesn't exist yet (first run before setup wizard)
    if let Err(e) = fs::create_dir_all(&paths.app_home).await {
        tracing::warn!(path = %paths.app_home.display(), error = %e,
            "could not create app home for log file, falling back to temp dir");
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
    // rest unchanged
}
```

If `resolve_paths` fails (no HOME set, unusual), we fall back gracefully without crashing.

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
| `crates/platform/src/tunnel/startup.rs` | `validate_named_tunnel_config_port`, before `fs::read_to_string` | `tracing::debug!` with `config_file` — e.g. `"reading cloudflare tunnel config for port validation"` |
| `crates/platform/src/tunnel/cloudflare_named.rs` | `start()`, around the `config_file.exists()` check (line 111) | `tracing::info!` confirming file found — e.g. `"cloudflare tunnel config file found"` with `config_file` |
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
tracing::debug!(config_file = %config_file.display(), "reading cloudflare tunnel config for port validation");

// cloudflare_named.rs::start — config file found
tracing::info!(config_file = %self.config_file.display(), "cloudflare tunnel config file found");
```

Note: no deletion logging is needed because brain3 never deletes the YAML.

---

## What does NOT change

- Log format, log level filtering, `RUST_LOG` env var — all unchanged
- The TUI "Logs" field already displays `launch_plan.log_file` — it will now just show the persistent path
- `tracing_appender::non_blocking` already opens in append mode — no change needed

---

## Out of scope

- Log rotation (can add `tracing-appender` rolling appender later)
- Surfacing old log location during migration (there's no migration; each run picks up the new path)
