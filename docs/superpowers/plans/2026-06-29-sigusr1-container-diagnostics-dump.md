# SIGUSR1 container diagnostics dump in the gateway host process

## Goal

Add a signal handler to the brain3 gateway host process so that, on receiving
`SIGUSR1`, it dumps the managed MCP container's logs (plus basic state) to the
process's own **stdout**. This makes container diagnostics capturable regardless
of who is driving the process — the E2E smoke test, or an operator debugging a
live install via `kill -USR1 $(pgrep brain3)`.

## Why the previous approaches failed

The managed MCP container is **owned by the brain3 gateway process** and is run
with Docker `--rm`. Lifecycle:

1. In the E2E test, the brain3 binary (`Brain3Process`) starts the container.
   When that process gets SIGINT, it runs `shutdown_managed_runtime()` →
   `stop_mcp_container()`. Because Docker containers always run with `--rm`
   (`crates/platform/src/container/startup.rs:319`), **stopping the container
   makes Docker immediately remove it.** The test then asserts
   `assert_no_container_residue()` — it *requires* the container to be gone.
2. In `apps/gateway/tests/e2e_smoke.rs`, the brain3 process is killed by
   `Brain3Process::drop` (sends `kill -INT`). On success this is the explicit
   `drop(gateway)`; on **any** failure the implicit drop does the same thing.

So by the time `cargo test` returns to `e2e_smoke.py`, and certainly by the time
a later `if: always()` CI step runs `docker logs brain3-mcp-vault-tools`, the
container no longer exists → "No such container" → empty output. A dump added to
`e2e_smoke.py` (the Python orchestrator) is always too late: container stoppage
is intermingled with brain3 shutdown, and the script never sees the container
alive.

The dump must therefore happen **inside the brain3 process, while it is still
running** (container still alive). A SIGUSR1 handler is the cleanest way to
trigger that on demand, and it's reusable outside tests.

## Grounding facts

- **`ContainerPort` already exposes `logs_tail(id, lines)`**
  (`crates/core/src/ports/container.rs:19`), already used for startup-failure
  diagnostics (`crates/core/src/application/ensure_container.rs:222`). No trait
  change needed.
- **The adapter is rebuildable from config** via
  `container_port_for_runtime(startup.runtime)`
  (`crates/platform/src/container/startup.rs:107`). Container name + runtime live
  in `runtime.config.container` (an `Arc<GatewayConfig>`). A signal handler needs
  only a clone of that `Arc`.
- **The gateway server runs until `shutdown_signal()` (ctrl_c)**
  (`apps/gateway/src/main.rs:658`). A SIGUSR1 listener just needs to run
  *concurrently* — a detached background task is enough.
- **Tracing does NOT go to stdout.** `init_logging` writes to a *log file* via a
  non-blocking appender, mirroring to **stderr** only when
  `enable_terminal_mirror()` is set (`apps/gateway/src/logging.rs:24-44`). The
  E2E test inherits both stdout and stderr. To *guarantee* the dump is visible to
  the test under `--nocapture`, the dump body goes to **stdout via `println!`**,
  with a `tracing::info!` marker so the log file also records that a dump
  happened.
- **`logs_tail` already blocks until `docker logs` finishes.** The Docker adapter
  implements it as `run_command("docker", &["logs", "--tail", N, id]).await`
  (`crates/platform/src/container/docker.rs:154-156`), and `run_command` uses
  tokio's `Command::…output().await`
  (`crates/platform/src/container/process.rs:11-12`), which awaits the child
  process to completion and collects all of its output. So inside brain3 the
  async dump task naturally waits for the full log capture before it prints and
  before it loops back to await the next signal. No extra waiting logic is needed
  *inside* brain3.

## Signal handler: synchronous or async?

Short answer: **we do not write a real OS signal handler at all, so ours is
async and can freely run `docker logs`.**

- A *real* Unix signal handler (installed via `sigaction` / the `signal-hook`
  crate) runs synchronously, asynchronously interrupting whatever the thread was
  doing, and may only call **async-signal-safe** functions. You cannot allocate,
  take a lock, `.await`, or spawn `docker logs` inside it — doing so is undefined
  behaviour. A real handler must therefore be tiny and synchronous (e.g. set an
  atomic flag or write one byte to a self-pipe).
- We instead use **`tokio::signal::unix::signal(SignalKind::user_defined1())`**.
  Tokio installs its own minimal async-signal-safe handler internally (it writes
  to a self-pipe/eventfd); your code runs later, in a **normal async task**, when
  `Signal::recv().await` resolves. That code is *not* in signal context and has
  none of the async-signal-safety restrictions — it can `.await`, allocate, and
  shell out to `docker logs` safely.

So our "handler" body is ordinary async code (`dump_container_diagnostics(...)
.await`) running on the tokio runtime. This is exactly why the design works and
why we must **not** reach for a raw synchronous signal handler — a synchronous
handler could not safely run `docker logs`.

## Implementation

### 1. Platform: dump function + signal listener

New file `crates/platform/src/runtime/diagnostics.rs` (keeps `main.rs` lean per
AGENTS.MD).

- `async fn dump_container_diagnostics(config: &GatewayConfig)`:
  - If `config.container` is `None`, log a one-liner and return.
  - Build the port via `container_port_for_runtime(startup.runtime)`,
    `id = ContainerId(container_name)`.
  - Call `port.logs_tail(&id, DIAGNOSTIC_LOG_LINES)` with a generous const
    (`10_000` — 2000 is not enough headroom). No trait change.
  - This is a **quick one-shot dump, not a follow.** `logs_tail` runs
    `docker logs --tail N` (never `--follow`/`-f`), so docker prints the last N
    lines and **exits immediately** — it does not stream or block. (Same as Unix
    `tail` without `-f`.) `run_command(...).output().await` just waits for that
    fast exit and collects the text.
  - `--tail` only *caps* how much we capture (last 10K lines; older lines are
    dropped). Failures show up near the end, so the cap is fine and is preferred
    over dumping the entire unbounded log. It's buffered into one `String` then
    printed; 10K lines of (quiet) MCP server logs is trivially small.
  - Also call `exists` / `is_running` so the dump notes container state when logs
    are empty.
  - Emit with banner delimiters to **stdout via `println!`**:
    `=== brain3 container diagnostics: <name> ===` … logs … `=== end ===`.
    Add one `tracing::info!(container, "dumped container diagnostics on SIGUSR1")`
    marker for the log file.

- `#[cfg(unix)] pub fn spawn_diagnostics_signal_listener(config: Arc<GatewayConfig>) -> tokio::task::JoinHandle<()>`:
  - `tokio::spawn` a loop:
    `let mut sig = signal(SignalKind::user_defined1())?;`
    then `while sig.recv().await.is_some() { dump_container_diagnostics(&config).await; }`.
  - `#[cfg(not(unix))]` no-op stub returning a resolved handle so it compiles
    off-unix (project is linux/macos-only; this is hygiene).
  - Detached: when `main` returns after server shutdown the runtime drops and the
    task is cancelled. No join needed.

### 2. main.rs: one call site

Right before `run_gateway_server_until` (`apps/gateway/src/main.rs:658`):

```rust
let _diag_listener = diagnostics::spawn_diagnostics_signal_listener(Arc::clone(&runtime.config));
```

That is the only addition to `main.rs`.

### 3. E2E test: signal **before** shutting down brain3

`apps/gateway/tests/e2e_smoke.rs`. The ordering we must guarantee is:

> SIGUSR1 (dump) → **wait until the dump has fully completed** → SIGINT
> (shutdown, which stops the container and lets `--rm` reap it).

If we shut down before the dump finishes, brain3's
`shutdown_managed_runtime()` could stop/remove the container out from under the
in-flight `docker logs`, yielding partial or failed output. So the test must
*wait* for the dump to finish — not guess with a fixed sleep.

#### How the test waits (handshake, no fixed sleep)

The dump's final stdout line is a **stable sentinel**, e.g.
`=== end brain3 container diagnostics: <name> ===` (this is part of the
human-readable banner anyway — no test-only hook in production code). The test
detects that line to know the dump is done:

- Spawn brain3 with **`Stdio::piped()` for stdout** (stderr stays `inherit()`).
- Immediately start a **reader thread** that continuously drains brain3's stdout,
  **tees** every line to the test process's own stdout (so `cargo test
  --nocapture` still shows everything, exactly as `inherit` did today), and
  watches for the sentinel. Draining from spawn-time also prevents brain3 from
  blocking on a full pipe buffer.
- When the reader sees the sentinel, it signals a `std::sync::mpsc` channel.
- `Brain3Process::dump_diagnostics(&self)` then becomes: send `kill -USR1 <pid>`,
  then **block on `recv_timeout` (e.g. 10s)** for the sentinel. This is fully
  synchronous, so it is safe to call from `Drop`. On timeout it logs a warning
  and proceeds (best-effort) rather than hanging the test.

(Alternative if we want to keep `Stdio::inherit()` unchanged: a **file
handshake** — the dump, when an env var like `B3_DIAGNOSTICS_DONE_FILE` is set,
appends a marker line to that file after finishing; the test polls the file.
Robust and thread-free, but it adds a small test-only affordance to production
code. The sentinel-on-stdout approach is preferred because it keeps production
free of test hooks.)

#### Dump on every run, before shutdown

- Replace the current direct-docker `dump_container_diagnostics` helper
  (currently `e2e_smoke.rs:600-630`) and its call sites with the signal-based
  `Brain3Process::dump_diagnostics()` above — the gateway now owns the logic.
- Make signalling the **normal flow**: a single RAII guard, declared *after*
  `gateway` so Rust drops it *first* (reverse declaration order), whose `Drop`
  always calls `gateway.dump_diagnostics()`. This fires on **all** exit paths —
  success, `?`-early-return, and panic — and always *before* `gateway`'s own
  `Drop` sends SIGINT. Because `dump_diagnostics` waits for the sentinel, the
  container is still alive throughout the dump; only afterward does `gateway`
  drop, send SIGINT, and let `--rm` reap the container. No disarm logic is needed
  (we want the logs on every run of an E2E smoke test).

### 4. Security / threat model

AGENTS.MD requires updating `SECURITY_AUDIT.MD` before new ingress. A SIGUSR1
handler is **local-only** — the sender must already have process control as the
same user (same trust boundary as the operator), *not* a network ingress.
Action items:

- Add a short Threat Model note: SIGUSR1 triggers a local diagnostics dump to the
  process's own stdout/log.
- Confirm the MCP container does not log secrets (it shouldn't, per AGENTS.MD)
  before relying on this.

## Trade-offs

- **Pro:** capability lives in the gateway, reusable by an operator on a live
  process, not just tests. The test stops shelling out to docker entirely.
- **Pro:** logs come through the same `ContainerPort` abstraction already used
  elsewhere — consistent, runtime-agnostic (works for macOS containers too).
- **Con:** slightly more moving parts (background task + signal plumbing) and a
  small fixed sleep in the test to let the async dump flush.
- **Con (note):** Windows needs the `cfg(not(unix))` stub; project is
  linux/macos-only so this is just hygiene.

## Open questions / knobs to confirm before implementing

- `SIGUSR1` vs `SIGUSR2`.
- `DIAGNOSTIC_LOG_LINES` cap (proposed 2000).
- Whether to also auto-dump on the gateway's own container-start-failure path
  (currently out of scope; startup failures already log tail via
  `ensure_container.rs`).

## Testing

- `cargo test` (full suite) must pass.
- `uv run scripts/e2e_smoke.py` — confirm the run is green **and** the container
  diagnostics (logs + sentinel line) appear in the cargo `--nocapture` output on
  the success path, emitted before shutdown.
- Temporarily break one assertion locally to confirm the failure/panic path still
  emits the dump (via SIGUSR1, through the guard's `Drop`) before teardown, and
  that the test waits on the sentinel rather than a fixed sleep.
- Manual: run brain3, `kill -USR1 $(pgrep brain3)`, confirm container logs appear
  on stdout, terminated by the sentinel line, while the process keeps running and
  the next SIGUSR1 dumps again.
