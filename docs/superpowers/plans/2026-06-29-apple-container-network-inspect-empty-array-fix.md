# Apple Container Network Inspect Empty Array Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix macOS native container network startup so `container network inspect <missing-network>` returning exit 0 plus `[]` is treated as missing, not as an incompatible existing network.

**Architecture:** Keep the fix inside the macOS container adapter. Add a small parser helper for Apple network inspect JSON and use it from `inspect_internal_network_state`; do not change Docker behavior, container ports, core domain errors, startup orchestration, or security model. Preserve fail-closed behavior for real existing networks that are not compatible with Brain3's internal network expectations.

**Tech Stack:** Rust, `serde_json::Value`, Tokio command execution through existing `run_command`, existing `ContainerPort` abstraction.

**Important repo constraints:** Do not commit. Do not use subagents. Run `cargo test` before considering the implementation complete.

---

## RCA Summary

The previous `exists()` fix addressed container inspection. Retesting exposed the same Apple CLI behavior on network inspection:

```bash
container network inspect brain3-dev-mcp-net
```

returned:

```json
[]
```

with exit code 0, while:

```bash
container network list
```

did not list `brain3-dev-mcp-net`.

Current code in `crates/platform/src/container/macos_container.rs` treats any successful `network inspect` command as an existing network. It then searches the output text for Docker-style fields:

```rust
"internal": true
"isInternal": true
```

Apple's inspect output for an existing internal macOS container network instead looks like:

```json
[
  {
    "id": "brain3-mcp-net",
    "state": "running",
    "config": {
      "labels": {},
      "pluginInfo": {
        "plugin": "container-network-vmnet",
        "variant": "reserved"
      },
      "id": "brain3-mcp-net",
      "mode": "hostOnly",
      "creationDate": 804003688.420691
    },
    "status": {
      "ipv6Subnet": "fd73:6958:a2bd:ef71::/64",
      "ipv4Gateway": "192.168.129.1",
      "ipv4Subnet": "192.168.129.0/24"
    }
  }
]
```

So there are two bugs in the macOS network compatibility path:

1. `[]` should mean `InternalNetworkState::Missing`.
2. Existing Apple internal networks should be recognized via `config.mode == "hostOnly"` and `config.pluginInfo.plugin == "container-network-vmnet"`, not only Docker-style `internal` booleans.

---

## File Structure

- Modify: `crates/platform/src/container/macos_container.rs`
  - Add a parser helper:
    - `fn parse_macos_network_inspect_state(output: &str) -> Result<InternalNetworkState, ContainerError>`
  - Update `inspect_internal_network_state()` to call the helper after `run_command`.
  - Add unit tests for missing, compatible, incompatible, legacy boolean-compatible, and malformed output.

No other files should change.

---

## Task 1: Add Parser Tests for macOS Network Inspect Output

**Files:**
- Modify: `crates/platform/src/container/macos_container.rs`

- [ ] **Step 1: Add derives needed for state assertions**

Change the enum near the top of `crates/platform/src/container/macos_container.rs` from:

```rust
enum InternalNetworkState {
    Missing,
    Compatible,
    Incompatible,
}
```

to:

```rust
#[derive(Debug, PartialEq, Eq)]
enum InternalNetworkState {
    Missing,
    Compatible,
    Incompatible,
}
```

- [ ] **Step 2: Add failing parser tests**

Append these tests inside the existing `#[cfg(test)] mod tests` block in `crates/platform/src/container/macos_container.rs`:

```rust
    #[test]
    fn parse_macos_network_inspect_state_treats_empty_array_as_missing() {
        assert_eq!(
            parse_macos_network_inspect_state("[]").expect("empty array should parse"),
            InternalNetworkState::Missing
        );
    }

    #[test]
    fn parse_macos_network_inspect_state_accepts_apple_hostonly_vmnet_network() {
        let output = r#"
[
  {
    "id": "brain3-mcp-net",
    "state": "running",
    "config": {
      "labels": {},
      "pluginInfo": {
        "plugin": "container-network-vmnet",
        "variant": "reserved"
      },
      "id": "brain3-mcp-net",
      "mode": "hostOnly",
      "creationDate": 804003688.420691
    },
    "status": {
      "ipv6Subnet": "fd73:6958:a2bd:ef71::/64",
      "ipv4Gateway": "192.168.129.1",
      "ipv4Subnet": "192.168.129.0/24"
    }
  }
]
"#;

        assert_eq!(
            parse_macos_network_inspect_state(output).expect("network inspect should parse"),
            InternalNetworkState::Compatible
        );
    }

    #[test]
    fn parse_macos_network_inspect_state_preserves_legacy_internal_boolean_support() {
        let output = r#"[{"name":"brain3-mcp-net","internal":true}]"#;

        assert_eq!(
            parse_macos_network_inspect_state(output).expect("network inspect should parse"),
            InternalNetworkState::Compatible
        );
    }

    #[test]
    fn parse_macos_network_inspect_state_rejects_existing_non_internal_network() {
        let output = r#"
[
  {
    "id": "default",
    "state": "running",
    "config": {
      "pluginInfo": {
        "plugin": "container-network-vmnet",
        "variant": "default"
      },
      "mode": "nat"
    }
  }
]
"#;

        assert_eq!(
            parse_macos_network_inspect_state(output).expect("network inspect should parse"),
            InternalNetworkState::Incompatible
        );
    }

    #[test]
    fn parse_macos_network_inspect_state_rejects_malformed_output() {
        let error = parse_macos_network_inspect_state("not json")
            .expect_err("malformed inspect output should be an error");

        assert!(matches!(error, ContainerError::Other(_)));
    }
```

- [ ] **Step 3: Run the targeted test command and confirm it fails**

Run:

```bash
cargo test -p brain3-platform parse_macos_network_inspect_state -- --nocapture
```

Expected before implementation:

```text
error[E0425]: cannot find function `parse_macos_network_inspect_state` in this scope
```

If the exact compiler diagnostic differs, the important expected state is: the new tests do not pass because the helper does not exist yet.

---

## Task 2: Implement macOS Network Inspect Parser

**Files:**
- Modify: `crates/platform/src/container/macos_container.rs`

- [ ] **Step 1: Add helper functions above `inspect_internal_network_state()`**

Insert this code after `enum InternalNetworkState`:

```rust
fn value_bool(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .any(|key| value.get(key).and_then(Value::as_bool).unwrap_or(false))
}

fn macos_network_entry_is_compatible(entry: &Value) -> bool {
    if value_bool(entry, &["internal", "Internal", "isInternal", "IsInternal"]) {
        return true;
    }

    let Some(config) = entry.get("config").or_else(|| entry.get("Config")) else {
        return false;
    };

    if value_bool(config, &["internal", "Internal", "isInternal", "IsInternal"]) {
        return true;
    }

    let mode = config
        .get("mode")
        .or_else(|| config.get("Mode"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let plugin = config
        .get("pluginInfo")
        .or_else(|| config.get("PluginInfo"))
        .and_then(|plugin_info| plugin_info.get("plugin").or_else(|| plugin_info.get("Plugin")))
        .and_then(Value::as_str)
        .unwrap_or_default();

    mode.eq_ignore_ascii_case("hostOnly") && plugin == "container-network-vmnet"
}

fn parse_macos_network_inspect_state(
    output: &str,
) -> Result<InternalNetworkState, ContainerError> {
    let value: Value = serde_json::from_str(output).map_err(|error| {
        ContainerError::Other(format!(
            "failed to parse macOS container network inspect output: {error}"
        ))
    })?;
    let entries = value.as_array().ok_or_else(|| {
        ContainerError::Other("macOS container network inspect output was not a JSON array".into())
    })?;

    if entries.is_empty() {
        return Ok(InternalNetworkState::Missing);
    }

    if entries.iter().any(macos_network_entry_is_compatible) {
        Ok(InternalNetworkState::Compatible)
    } else {
        Ok(InternalNetworkState::Incompatible)
    }
}
```

- [ ] **Step 2: Run targeted tests and confirm they pass**

Run:

```bash
cargo test -p brain3-platform parse_macos_network_inspect_state -- --nocapture
```

Expected:

```text
test result: ok. 5 passed; 0 failed
```

---

## Task 3: Wire Parser into Network State Inspection

**Files:**
- Modify: `crates/platform/src/container/macos_container.rs`

- [ ] **Step 1: Replace the current string-normalization logic**

Change `inspect_internal_network_state()` from:

```rust
async fn inspect_internal_network_state(
    name: &str,
) -> Result<InternalNetworkState, ContainerError> {
    match run_command("container", &["network", "inspect", name]).await {
        Ok(out) => {
            let normalized: String = out
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            if normalized.contains("\"internal\":true")
                || normalized.contains("\"isinternal\":true")
            {
                Ok(InternalNetworkState::Compatible)
            } else {
                Ok(InternalNetworkState::Incompatible)
            }
        }
        Err(ContainerError::CommandFailed { .. }) => Ok(InternalNetworkState::Missing),
        Err(e) => Err(e),
    }
}
```

to:

```rust
async fn inspect_internal_network_state(
    name: &str,
) -> Result<InternalNetworkState, ContainerError> {
    match run_command("container", &["network", "inspect", name]).await {
        Ok(out) => {
            // Apple `container network inspect <missing-name>` can exit 0 and
            // print `[]`. Parse the JSON instead of trusting the exit status so
            // a missing network is created instead of reported as a false
            // incompatible-network conflict.
            parse_macos_network_inspect_state(&out)
        }
        Err(ContainerError::CommandFailed { .. }) => Ok(InternalNetworkState::Missing),
        Err(e) => Err(e),
    }
}
```

- [ ] **Step 2: Run the targeted tests again**

Run:

```bash
cargo test -p brain3-platform parse_macos_network_inspect_state -- --nocapture
```

Expected:

```text
test result: ok. 5 passed; 0 failed
```

---

## Task 4: Verify Full Workspace

**Files:**
- No file changes in this task.

- [ ] **Step 1: Run full test suite**

Run:

```bash
cargo test
```

Expected:

```text
test result: ok
```

There are multiple test binaries in this workspace, so verify every reported test binary exits successfully.

- [ ] **Step 2: Review final diff**

Run:

```bash
git diff -- crates/platform/src/container/macos_container.rs
```

Expected diff shape:

- `InternalNetworkState` derives `Debug`, `PartialEq`, and `Eq`.
- New parser helper functions exist in `macos_container.rs`.
- `inspect_internal_network_state()` calls `parse_macos_network_inspect_state(&out)`.
- Existing `MacOsContainerAdapter::exists()` workaround remains unchanged.
- No Docker files, core files, gateway files, security docs, or environment files are modified.

- [ ] **Step 3: Optional manual verification after tests**

If running on macOS with Apple container permissions, manually verify the missing-network case:

```bash
container network inspect brain3-dev-mcp-net
```

If it returns:

```json
[]
```

then launch Brain3 with:

```bash
BRAIN3_HOME=/Users/tleyden/.brain3_dev cargo run -p brain3
```

Expected runtime behavior:

- Brain3 should not report `container network name 'brain3-dev-mcp-net' already exists`.
- It should attempt `container network create --internal brain3-dev-mcp-net`.
- If network creation fails for a real Apple CLI reason, the new error should be different and should come from the create command, not from the false compatibility check.

---

## Implementation Notes

- Keep this macOS-only. Docker already uses `docker network inspect --format "{{.Internal}}"` and handles missing networks through a non-zero command exit.
- Keep malformed `network inspect` JSON as an error instead of silently treating it as missing. A malformed successful response means the runtime contract changed or the command output is not what we expect, and startup should fail loudly.
- Keep incompatible existing networks as conflicts. If a user has a real network with the same name but `mode != "hostOnly"` or a different plugin, Brain3 should not reuse it.
- The compatibility rule is intentionally conservative:

```rust
mode.eq_ignore_ascii_case("hostOnly") && plugin == "container-network-vmnet"
```

This matches observed Apple internal network output and avoids treating the default NAT network as compatible.

