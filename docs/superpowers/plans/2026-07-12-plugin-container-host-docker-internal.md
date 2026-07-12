# Fix: Plugin MCP containers can't reach `host.docker.internal` on Linux Docker

## Symptom

`tools/list` only returns the built-in vault tools (and `transcribe_audio_file`)
— none of a configured plugin's tools show up, even though the plugin is
correctly defined in `~/.brain3_dev/brain3.yaml`. There's no error in the
MCP response; the tool count is just quietly smaller than expected.

## Root cause

`brain3` does start the plugin's container, but in the case that surfaced
this bug the container's process crashed roughly 7 seconds after boot. The
crash log showed an uncaught exception on startup while the plugin's own
process tried to reach a backend service over `http://host.docker.internal:<port>`:

```
TypeError: fetch failed
Caused by: Error: getaddrinfo EAI_AGAIN host.docker.internal (EAI_AGAIN)
```

`host.docker.internal` is a convenience DNS name that Docker Desktop
(macOS/Windows) resolves for you automatically via its embedded DNS server —
any container can reach the host through it with zero extra configuration.
Native Linux Docker has no such embedded DNS server and does not add that
entry to a container's `/etc/hosts` or resolver unless the container is
started with `--add-host=host.docker.internal:host-gateway` (which maps the
name to the special `host-gateway` address, i.e. the Docker bridge gateway
IP that routes back to the host). `brain3`'s plugin containers are never
started with that flag, so on Linux any plugin that calls out to
`host.docker.internal` fails DNS resolution and — depending on how well the
plugin's own code handles that — can crash outright on boot.

Two things compounded to make this hard to diagnose from the MCP client
side:

1. **The failure is silent by design.** When a plugin container fails to
   start, `brain3` logs "Plugin MCP Container startup failed; continuing
   without it" and keeps running with that plugin's tools simply absent
   from `tools/list`. That's intentional — one broken plugin shouldn't take
   down the whole server, and the failure is logged, so it's diagnosable
   from `brain3.log` even though it's invisible to the MCP client. This
   plan doesn't touch that behavior.
2. **The plugin itself has no host-reachability workaround.** Even once the
   `brain3`-side fix below is in place, any plugin that hits an uncaught
   exception rather than handling a failed fetch gracefully will still take
   its whole process down on any transient host-connectivity hiccup — a
   plugin-side concern, out of scope for this `brain3` plan.

## Root cause location

- `crates/platform/src/container/startup.rs:342` `build_plugin_container_config()` builds the `ContainerConfig` for every plugin container but never sets anything to make `host.docker.internal` resolvable.
- `crates/platform/src/container/docker.rs:221` `DockerContainerAdapter::run()` builds the `docker run` arg vector and has no `--add-host` handling at all.
- `crates/core/src/domain/model.rs:143` `ContainerConfig` has no field to carry this through.

## Changes

### 1. `crates/core/src/domain/model.rs`
Add a new field to `ContainerConfig` (~line 143-161):
```rust
pub extra_hosts: Vec<String>,  // e.g. "host.docker.internal:host-gateway"
```
Each entry is a raw `host:ip-or-special-value` string, passed straight through to `--add-host <entry>`.

### 2. `crates/platform/src/container/startup.rs`
- `build_plugin_container_config()` (~line 384): set
  `extra_hosts: match plugin.runtime { ContainerRuntime::Docker => vec!["host.docker.internal:host-gateway".into()], ContainerRuntime::MacOSContainer => Vec::new() }`.
  (Harmless to also send it on macOS Docker Desktop, but there's no reason to — Desktop already resolves the name, and `ContainerRuntime::MacOSContainer` is Apple's native `container` CLI, which doesn't support `--add-host` the same way, so it must stay empty there.)
- Vault-tools container config builder (~line 247-336, the other `ContainerConfig { ... }` in this file): set `extra_hosts: Vec::new()` — not affected by this bug, no known need for it.

### 3. `crates/platform/src/container/docker.rs`
In `DockerContainerAdapter::run()` (~line 221), add a loop alongside the existing `--env`/`--label` loops:
```rust
for host in &config.extra_hosts {
    args.push("--add-host".into());
    args.push(host.clone());
}
```

### 4. `crates/platform/src/container/macos_container.rs`
No behavior change needed — `MacOsContainerAdapter::run()` (~line 314) can be left alone since `extra_hosts` will always be empty for `ContainerRuntime::MacOSContainer`. Not worth adding dead code there.

### 5. Other `ContainerConfig { .. }` construction sites (compile fixes only)
- `crates/core/src/application/ensure_container.rs:468` (`sample_config()` test helper) — add `extra_hosts: Vec::new()`.

### 6. Tests to update
- `crates/platform/src/container/startup.rs`:
  - `build_plugin_container_config_adds_plugin_role_labels_and_mounts` (~line 832): add an assertion that `config.extra_hosts == vec!["host.docker.internal:host-gateway"]` when `runtime: ContainerRuntime::Docker` (the existing `sample_plugin_config()` at line 816 already uses `ContainerRuntime::Docker`, so this is a direct addition).
  - Consider adding a second case (or a small parametrized check) confirming `extra_hosts` is empty for `ContainerRuntime::MacOSContainer`.
- `crates/platform/tests/setup_bootstrap.rs`: no `ContainerConfig` literals here, so nothing to change beyond making sure it still compiles.
- `crates/platform/src/container/docker.rs`: no existing unit test exercises `run()`'s arg-building (it shells out via `run_command`), so there's nothing currently asserting the docker arg list. Not adding new test scaffolding for this unless you want to refactor arg-building into a separately testable pure function — flagging as optional, out of scope for a minimal fix.

## Explicitly not doing (deferred)

- **Per-plugin env var / config overrides.** `PluginMcpContainerConfig` (`crates/core/src/domain/model.rs:69`) and `build_plugin_container_config()` still hard-code `env_vars: Vec::new()`, so there's no way to override a plugin's own config (e.g. a backend URL it calls out to) per-deployment without rebuilding the plugin image. Not required to fix this crash (the crash is a DNS resolution failure, not a bad URL), so leaving it out. Worth a separate plan if plugins need per-install config injection.

## Verification

1. `cargo test -p brain3 --no-run`
2. `cargo test` — check the two updated/added `startup.rs` assertions
3. Rebuild the `brain3` binary locally, point it at a `brain3.yaml` with a plugin MCP container configured with `runtime: docker` on Linux, and confirm the plugin container survives past boot and its tools show up in `tools/list`.
