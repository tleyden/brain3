# Fix: Log Path Ignores --brain3-home Override

## Problem

When launching with `--brain3-home ~/.brain3_dev`, the TUI shows the log file under
`~/.brain3/brain3.log` instead of `~/.brain3_dev/brain3.log`.

## Root Cause

`init_logging` in `apps/gateway/src/logging.rs:73` constructs `PlatformSetupSystem::new()`,
which sets `app_home_override: None`. With no override, `resolve_paths()` falls through to
`Brain3AppHome::resolve_from_env()` which always returns `~/.brain3` (the hardcoded default).

The `--brain3-home` CLI arg is parsed in `main()` and stored in `args.brain3_home`, but
`init_logging` is called on line 673 with only `args.log_level.as_str()` — the home override
is never forwarded.

**Execution path:**
1. `main()` parses args → `args.brain3_home = Some("~/.brain3_dev")`
2. `init_logging(args.log_level.as_str())` called — home arg dropped
3. `logging.rs`: `PlatformSetupSystem::new()` → `app_home_override = None`
4. `resolve_paths()` → `Brain3AppHome::resolve_from_env()` → `~/.brain3`
5. `resolve_log_file(paths)` → `~/.brain3/brain3.log` ← **wrong**

## Fix

Three small edits across two files.

### 1. Update `init_logging` signature — `apps/gateway/src/logging.rs:72`

```rust
// before
pub async fn init_logging(default_level: &str) -> Result<GatewayLogging>

// after
pub async fn init_logging(default_level: &str, brain3_home: Option<PathBuf>) -> Result<GatewayLogging>
```

### 2. Use the override when constructing `PlatformSetupSystem` — `logging.rs:73`

```rust
// before
let setup_system = PlatformSetupSystem::new();

// after
let setup_system = match brain3_home {
    Some(dir) => PlatformSetupSystem::with_home_override(dir),
    None => PlatformSetupSystem::new(),
};
```

### 3. Pass the home arg from `main()` — `apps/gateway/src/main.rs:673`

```rust
// before
let logging = logging::init_logging(args.log_level.as_str()).await?;

// after
let logging = logging::init_logging(args.log_level.as_str(), args.brain3_home.clone()).await?;
```

## Scope

No test changes needed — this is a startup initialization path with no existing unit tests.
