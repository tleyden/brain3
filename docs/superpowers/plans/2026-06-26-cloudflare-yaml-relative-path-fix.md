# Fix: Cloudflare Named Tunnel Config YAML Written to CWD Instead of Brain3 Home

## RCA

**Root cause: `crates/platform/src/config/env_file.rs:472-473` uses a relative fallback path.**

When `B3_CF_TUNNEL_CONFIG_FILE` is empty or missing from `.env`, the fallback is:

```rust
PathBuf::from(format!(".cloudflared/{tunnel_name}.yml"))
```

This is a relative path that resolves against the process's working directory. If the user runs brain3 from `/home/tleyden/brain3_dev`, the config YAML lands at `/home/tleyden/brain3_dev/.cloudflared/brain3-dev.yml` instead of `~/.brain3/cloudflared/brain3-dev.yml`.

**Why the env var is sometimes empty:** The setup wizard (`env_writer.rs:140-147`) correctly writes the full absolute path `paths.cloudflared_dir.join("{tunnel_name}.yml")` into `B3_CF_TUNNEL_CONFIG_FILE`. But if the `.env` predates that field being added, or was manually edited to blank it out, the read-time fallback in `env_file.rs` triggers and uses the wrong relative path.

**What is not a bug:** `find_credentials_file` in `cloudflare_setup.rs:101-102` hardcodes `~/.cloudflared/{id}.json`. That is correct — `cloudflared` always stores its credentials there and brain3 cannot change it.

---

## Plan

**Single change in `crates/platform/src/config/env_file.rs`.**

Replace the relative fallback (lines 472-473) with one derived from `Brain3AppHome::resolve_from_env()`:

```rust
// Before
let config_file = if config_file_str.is_empty() {
    PathBuf::from(format!(".cloudflared/{tunnel_name}.yml"))
} else {
    PathBuf::from(config_file_str)
};

// After
let config_file = if config_file_str.is_empty() {
    let app_home = Brain3AppHome::resolve_from_env()?;
    app_home.cloudflared_dir.join(format!("{tunnel_name}.yml"))
} else {
    PathBuf::from(config_file_str)
};
```

The default becomes `~/.brain3/cloudflared/{tunnel_name}.yml` (or `$B3_HOME/cloudflared/{tunnel_name}.yml`), matching exactly what the setup wizard writes. If `HOME` is unset, `resolve_from_env()` returns a `SetupError` which propagates up — no silent fallback to a relative path.

No other files need to change.
