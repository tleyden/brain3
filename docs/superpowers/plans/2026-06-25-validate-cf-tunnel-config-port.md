# Plan: Validate Cloudflare Named Tunnel Config Port on Startup

## Problem

The cloudflare named tunnel config YAML (`.cloudflared/brain3.yml`) has an ingress
`service` URL like `http://localhost:8521`. If that port doesn't match the port the
gateway is actually bound to (`B3_OAUTH2_GATEWAY_PORT`, default `2763`), the tunnel
silently forwards traffic to the wrong port and OAuth discovery breaks.

## Root Cause

`start_tunnel()` in `crates/platform/src/tunnel/startup.rs` constructs
`CloudflareNamedTunnelAdapter` from `TunnelConfig::CloudflareNamed` but ignores
`local_port` (it uses `..` in the match arm). The adapter gets the config file path
but never compares the gateway port against what the YAML specifies.

## What We Already Have

- `TunnelConfig::CloudflareNamed { local_port, config_file, .. }` — has both the
  gateway port (from env) and the path to the YAML file.
- `TunnelError` in `crates/core/src/domain/errors.rs` — the right place to add a new
  variant for this mismatch.
- `start_tunnel()` in `startup.rs` — the natural choke-point to add validation before
  the adapter is constructed.

## Cloudflare Config YAML Format

```yaml
tunnel: <uuid>
credentials-file: /path/to/credentials.json

ingress:
  - hostname: brain3.mcpnative.dev
    service: http://localhost:2763    # <-- this port must match gateway port
  - service: http_status:404          # catch-all, not a real service URL
```

## Plan

### Step 1 — Add `TunnelError::PortMismatch` variant

File: `crates/core/src/domain/errors.rs`

Add to the `TunnelError` enum:

```rust
#[error(
    "cloudflare tunnel config port mismatch: config routes to port {config_port}, \
     but gateway is on port {gateway_port} — update the 'service' URL in {config_file}"
)]
PortMismatch {
    config_port: u16,
    gateway_port: u16,
    config_file: String,
},
```

### Step 2 — Add `validate_named_tunnel_config_port()` in `startup.rs`

File: `crates/platform/src/tunnel/startup.rs`

New function (no new crate dependencies — parse the URL manually from the string):

```rust
/// Parses the cloudflare named tunnel YAML and checks that at least one non-catch-all
/// ingress rule targets `expected_port`. Returns `TunnelError::PortMismatch` if every
/// rule that IS a real service URL points to a different port.
fn validate_named_tunnel_config_port(
    config_file: &std::path::Path,
    expected_port: u16,
) -> Result<(), TunnelError> {
    // Read the file (sync is fine — called during bootstrap, before tokio tasks depend on it)
    let content = std::fs::read_to_string(config_file).map_err(|e| {
        TunnelError::Other(format!(
            "failed to read tunnel config {}: {e}",
            config_file.display()
        ))
    })?;

    // Walk lines looking for `service: http[s]://...` entries.
    // Skip the http_status catch-all.
    let mut found_service = false;
    for line in content.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("service:") else {
            continue;
        };
        let url = rest.trim();
        if url.starts_with("http_status:") {
            continue; // catch-all rule, skip
        }
        found_service = true;

        // Extract port from "http[s]://host:PORT" — split on ':' and take the last segment.
        if let Some(port_str) = url.rsplitn(2, ':').next() {
            // strip any trailing path component e.g. ":2763/path"
            let port_str = port_str.split('/').next().unwrap_or(port_str);
            if let Ok(config_port) = port_str.parse::<u16>() {
                if config_port == expected_port {
                    return Ok(()); // at least one rule matches — we're good
                }
                return Err(TunnelError::PortMismatch {
                    config_port,
                    gateway_port: expected_port,
                    config_file: config_file.display().to_string(),
                });
            }
        }
    }

    if !found_service {
        tracing::warn!(
            config_file = %config_file.display(),
            "no 'service:' lines found in tunnel config — skipping port validation"
        );
    }

    Ok(())
}
```

### Step 3 — Call it in `start_tunnel()`

File: `crates/platform/src/tunnel/startup.rs`

Change the `CloudflareNamed` match arm from:

```rust
TunnelConfig::CloudflareNamed {
    tunnel_name,
    domain,
    config_file,
    ..
} => Box::new(CloudflareNamedTunnelAdapter::new(
    tunnel_name,
    domain,
    config_file.clone(),
    pid_file,
)),
```

To:

```rust
TunnelConfig::CloudflareNamed {
    tunnel_name,
    domain,
    config_file,
    local_port,
} => {
    validate_named_tunnel_config_port(config_file, *local_port)?;
    Box::new(CloudflareNamedTunnelAdapter::new(
        tunnel_name,
        domain,
        config_file.clone(),
        pid_file,
    ))
}
```

### Step 4 — Tests

Add unit tests at the bottom of `startup.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_tunnel_config(service_url: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "tunnel: test-uuid").unwrap();
        writeln!(f, "ingress:").unwrap();
        writeln!(f, "  - hostname: brain3.example.dev").unwrap();
        writeln!(f, "    service: {service_url}").unwrap();
        writeln!(f, "  - service: http_status:404").unwrap();
        f
    }

    #[test]
    fn matching_port_passes() {
        let f = write_tunnel_config("http://localhost:2763");
        assert!(validate_named_tunnel_config_port(f.path(), 2763).is_ok());
    }

    #[test]
    fn mismatched_port_returns_error() {
        let f = write_tunnel_config("http://localhost:8521");
        let err = validate_named_tunnel_config_port(f.path(), 2763).unwrap_err();
        assert!(matches!(err, TunnelError::PortMismatch { config_port: 8521, gateway_port: 2763, .. }));
    }

    #[test]
    fn only_catch_all_no_error() {
        // Config with no real service entry — warn but don't fail
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "ingress:").unwrap();
        writeln!(f, "  - service: http_status:404").unwrap();
        assert!(validate_named_tunnel_config_port(f.path(), 2763).is_ok());
    }
}
```

Note: needs `tempfile` in `[dev-dependencies]` of `crates/platform/Cargo.toml` if not
already there. Check first with `grep tempfile crates/platform/Cargo.toml`.

## Files Changed

| File | Change |
|------|--------|
| `crates/core/src/domain/errors.rs` | Add `TunnelError::PortMismatch` variant |
| `crates/platform/src/tunnel/startup.rs` | Add `validate_named_tunnel_config_port()`, call it in `start_tunnel()`, add tests |
| `crates/platform/Cargo.toml` | Add `tempfile` to dev-dependencies if missing |

## No New Runtime Dependencies

Parsing is done with `std::fs` + string splitting. No new crates needed at runtime.

## Error Message Seen by the User

```
ERROR: cloudflare tunnel config port mismatch: config routes to port 8521,
but gateway is on port 2763 — update the 'service' URL in .cloudflared/brain3.yml
```

Brain3 exits before launching cloudflared, rather than silently failing at OAuth time.
