# Plan: Generous, non-blocking container startup probe

Date: 2026-06-30

## Background / RCA

Brain3 v0.2.8 aborted startup because the MCP container did not accept TCP
connections on its container IP (`172.18.0.2:2765`) within the 5-second probe
window.

v0.2.8 introduced a synchronous, blocking frontmatter index build that runs
*before* Uvicorn starts listening on port 2765. On a 2,373-file vault this scan
takes ~4.5s; combined with Python cold start (~2s), the port opens at ~6.5-7s
after `docker run` — about 1.5-2s past the hardcoded 5s probe budget. The
container itself is healthy (binds `0.0.0.0:2765`, correct IP); the probe simply
gave up too early.

We are intentionally keeping the blocking scan (so the index is fresh at
startup). This plan only makes startup patient enough — and observable enough —
to absorb it.

### Two sequential 5s gates exist in the startup path (`bootstrap.rs`)

1. **Gate 1 — TCP reachability** (`crates/core/src/application/ensure_container.rs`,
   `DEFAULT_STARTUP_TIMEOUT = 5s`). The gate that failed.
2. **Gate 2 — MCP functional probe** (`crates/platform/src/runtime/health_probe.rs`,
   `PROBE_TOTAL_TIMEOUT = 5s`, `PROBE_MAX_ATTEMPTS = 7`). Runs *after* Gate 1
   passes, doing auth + a real `vault list` RPC.

### Concurrency hazard

The Gate 1 probe loop is an `async fn` but uses `std::thread::sleep`
(`ensure_container.rs:210`) and blocking `std::net::TcpStream::connect_timeout`
(`ensure_container.rs:257`). At 5s this is tolerable; at 2 minutes it would
freeze the TUI and stall the tokio runtime for the entire wait. Raising the
timeout therefore *requires* making the loop non-blocking.

## Changes

### 1. Raise the give-up limit (Gate 1)
`crates/core/src/application/ensure_container.rs:13`
- `DEFAULT_STARTUP_TIMEOUT`: `5s` -> **`120s`**.
- Keep `DEFAULT_STARTUP_POLL_INTERVAL = 200ms` (fast detection once the port
  opens; <=600 polls over 120s once the loop is non-blocking).

### 2. Make the probe loop non-blocking (required by #1)
`verify_startup` loop (lines ~170-211) and `tcp_port_ready` (lines ~253-260):
- Replace `std::thread::sleep` -> `tokio::time::sleep(...).await`.
- Replace the blocking TCP connect: wrap the existing `tcp_port_ready` in
  `tokio::task::spawn_blocking` (preferred — smallest diff, keeps
  `to_socket_addrs` / DNS behavior identical), or convert to
  `tokio::net::TcpStream::connect` under `tokio::time::timeout`.
- Drop the now-unused `use std::thread::sleep`.
- The `is_running().await` check at the top of the loop stays — it already gives
  the fast "container exited" failure path so a real crash fails immediately
  instead of waiting the full 120s.

### 3. Periodic progress logging during the wait
Today the loop logs once at start, then nothing until success/timeout — a silent
up-to-2-minute hang reads as a freeze in the TUI.
- Add an `INFO` heartbeat every ~5s with `container`, `elapsed_s`, `timeout_s`,
  message "waiting for container to become reachable". Cheap, and makes the
  blocking-scan wait legible in `brain3.log`.

### 4. Gate 2 modest bump (`health_probe.rs`)
Runs after the server is already listening, so 5s for one `vault list` RPC is
usually fine — but on a large vault the first RPC can be heavier, and it is the
same brittle hardcoded-5s pattern.
- `PROBE_TOTAL_TIMEOUT`: `5s` -> **`30s`** (leave `PROBE_MAX_ATTEMPTS = 7`).
  Headroom without waiting minutes on a functional RPC that should answer in well
  under a second once the server is up.

## Out of scope
- The v0.2.8 blocking frontmatter scan stays as-is. No async/deferred index
  build, no change to vault-tools.

## Tests & verification
- Existing unit tests (`ensure_container.rs:603,645`) drive failures via
  `with_probe_settings` at a 30ms timeout, so they are independent of the new
  120s default and stay fast/green. The async-sleep change keeps them on the
  existing tokio test harness.
- `cargo test -p brain3 --no-run` then `cargo test`.
- This touches container startup, so also run E2E smoke before calling it done:
  `uv run scripts/e2e_smoke.py`.
