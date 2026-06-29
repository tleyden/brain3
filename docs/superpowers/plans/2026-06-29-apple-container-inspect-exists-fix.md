# Fix: Apple container CLI `inspect` exits 0 for non-existent containers

## Problem

`exists()` in `crates/platform/src/container/macos_container.rs:160-162` uses
`command_succeeds("container", &["inspect", &id.0])` which only checks the exit code:

```rust
async fn exists(&self, id: &ContainerId) -> Result<bool, ContainerError> {
    command_succeeds("container", &["inspect", &id.0]).await
}
```

Apple's `container` CLI has a bug: `container inspect <nonexistent-name>` exits 0 and
prints `[]` (an empty JSON array) instead of exiting non-zero like Docker does.
`command_succeeds` sees exit 0 → returns `true` → `exists()` says the container exists
→ `ensure_container.rs:77` raises `ContainerError::Conflict` even though the container
was never created.

## Fix (single file, single function)

Replace the `exists()` implementation so it calls `run_command` (which returns the
stdout string) and then checks whether the returned JSON array is non-empty.

**File:** `crates/platform/src/container/macos_container.rs`

**Change `exists()` from:**
```rust
async fn exists(&self, id: &ContainerId) -> Result<bool, ContainerError> {
    command_succeeds("container", &["inspect", &id.0]).await
}
```

**To:**
```rust
async fn exists(&self, id: &ContainerId) -> Result<bool, ContainerError> {
    match run_command("container", &["inspect", &id.0]).await {
        Ok(out) => {
            // Apple container CLI exits 0 with [] for non-existent containers.
            // Check the array is non-empty to distinguish real containers.
            let json: serde_json::Value = serde_json::from_str(&out)
                .unwrap_or(serde_json::Value::Array(vec![]));
            Ok(json.as_array().map_or(false, |arr| !arr.is_empty()))
        }
        Err(ContainerError::CommandFailed { .. }) => Ok(false),
        Err(e) => Err(e),
    }
}
```

`serde_json::Value` is already imported at the top of the file (`use serde_json::Value`).
The `CommandFailed` arm handles Docker's behaviour (exits non-zero for missing containers)
and any future Apple CLI fix. Only a `SpawnFailed` or other hard error propagates.

You can drop the `command_succeeds` import from `super::process` if it is no longer
referenced elsewhere in the file (check after the change; `image_exists` also uses it,
so the import stays).

## Tests

Add a unit test in the `#[cfg(test)]` block at the bottom of `macos_container.rs` that
exercises the two cases the new logic must distinguish:

```rust
#[test]
fn exists_returns_false_for_empty_json_array() {
    // Simulates Apple container CLI bug: exit 0 + "[]"
    // parse_macos_inspect_output([]) → empty vec
    let containers = parse_macos_inspect_output("[]").expect("empty array should parse");
    assert!(containers.is_empty());
}
```

The real async path is covered by the existing `MockContainerPort` in `ensure_container.rs`
tests. No new async integration test is needed.

Run `cargo test` after the change to confirm nothing regresses.

## Scope

- One function changed: `MacOsContainerAdapter::exists` in `macos_container.rs`
- No changes to `ensure_container.rs`, `process.rs`, ports, or domain errors
- No behaviour change on Docker (Docker exits non-zero → `CommandFailed` arm → `false`)
