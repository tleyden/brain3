# Plan: native tool `_meta` passthrough + bounded audio download

Fixes two P1 review findings on the native Whisper transcription tool
(`crates/platform/src/native_mcp_tools/whisper_transcribe.rs`) and the
registry that advertises it
(`crates/core/src/application/native_mcp_tool_registry.rs`).

## Issue 1: missing `openai/fileParams` metadata in `tools/list`

The container tool `save_audio_file` (see
`brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py`, historical
commit `dc1003b`) advertises itself to OpenAI clients via:

```python
@mcp.tool(
    name="save_audio_file",
    ...
    meta={"openai/fileParams": ["audio_file"]},
)
```

The Python MCP SDK serializes `meta` as the wire field `_meta` on the `Tool`
object (`mcp/types.py`: `meta: dict[str, Any] | None = Field(alias="_meta")`).
OpenAI-side clients read `_meta["openai/fileParams"]` to know which input
property should be bound to an uploaded file, rather than treated as a plain
JSON argument.

`NativeMcpToolRegistry::list_schemas()`
(`crates/core/src/application/native_mcp_tool_registry.rs:20`) builds native
tool JSON schemas by hand and only emits `name`/`description`/`inputSchema` —
there is no `_meta` key. `WhisperTranscribeTool` takes the same
`audio_file` OpenAI file reference shape as `save_audio_file` but, because
it's a native tool, it never gets the `_meta` annotation, so OpenAI clients
will list it as a normal tool and never upload/bind a file to it. The tool
is effectively unusable from ChatGPT.

### Fix

1. Add an optional `meta()` hook to the `NativeMcpTool` port
   (`crates/core/src/ports/native_mcp_tool.rs`), defaulting to `None`, e.g.:

   ```rust
   fn meta(&self) -> Option<Value> {
       None
   }
   ```

2. In `NativeMcpToolRegistry::list_schemas()`, include `"_meta"` in the
   emitted JSON only when `tool.meta()` returns `Some(...)`, e.g. build the
   base `json!({...})` object and then `.as_object_mut().unwrap().insert("_meta".into(), meta)` conditionally, so tools without metadata keep the
   current lean output (no `"_meta": null` noise).

3. Override `meta()` in `WhisperTranscribeTool`
   (`crates/platform/src/native_mcp_tools/whisper_transcribe.rs`) to return:

   ```rust
   Some(json!({ "openai/fileParams": ["audio_file"] }))
   ```

   matching the container tool's `openai/fileParams` value exactly (the
   input property name that carries the OpenAI file reference).

4. Update/add a unit test in `whisper_transcribe.rs`'s `#[cfg(test)] mod
   tests` asserting `tool.meta()` (or the registry's `list_schemas()` output
   for this tool) contains `"_meta": {"openai/fileParams": ["audio_file"]}`.
   Follow the existing `input_schema_declares_openai_audio_file_reference`
   test pattern — this is behavior (what gets advertised to callers), not a
   description-string test, so it's fair game per AGENTS.MD testing rules.

## Issue 2: unbounded download duration in `download_to_temp_file`

`WhisperTranscribeTool::with_transcriber`
(`crates/platform/src/native_mcp_tools/whisper_transcribe.rs:44-47`) builds
the `reqwest::Client` with only `.no_proxy()` — no connect timeout and no
overall/per-read timeout. The byte cap in `download_to_temp_file`
(lines ~143-158) only rejects a download once bytes have actually arrived
and exceeded `max_audio_bytes`; a malicious or slow `download_url` (which is
attacker-influenced input via an authorized `tools/call`) can accept the
TCP connection and then never send data, or trickle bytes slower than the
cap is reached, tying up the gateway's async task (and the `TempDir`/socket)
indefinitely.

### Fix

1. Add explicit timeouts to the `reqwest::Client` builder in
   `WhisperTranscribeTool::with_transcriber`:

   ```rust
   let http_client = reqwest::Client::builder()
       .no_proxy()
       .connect_timeout(Duration::from_secs(10))
       .timeout(Duration::from_secs(300))
       .build()
       .expect("failed to build reqwest client for native transcription tool");
   ```

   - `connect_timeout` bounds the TCP/TLS handshake.
   - `timeout` is reqwest's *total request timeout* (covers the whole
     `send()` + body streaming), which directly closes the stall gap the
     byte cap doesn't cover — a stalled or drip-fed response will now be
     aborted once the total time budget is exceeded, regardless of bytes
     received so far.
   - Pick concrete constants — 300s total is generous for the size caps
     already in place (`max_audio_bytes`), but should be reviewed against
     the actual configured `max_audio_bytes` / expected network conditions
     rather than copied verbatim. If `WhisperTranscribeConfig` doesn't
     already have a place for tuning this, either hardcode reasonable
     constants near `TARGET_SAMPLE_RATE` or add a config field — prefer
     hardcoding unless there's already a config knob pattern in this file
     for similar constants (there is: `TARGET_SAMPLE_RATE`).

2. Add a regression test using the existing `serve_once` test helper
   pattern: spin up an axum handler that never writes a body / stalls
   indefinitely (or sleeps past a short test-configured timeout), and
   assert the tool call fails with a `Download` error within a bounded
   wall-clock time instead of hanging. Because the production timeout
   constants above are multi-minute, the test will need either:
   - a way to inject a shorter timeout for the test client (e.g. extend
     `with_transcriber` or add a test-only constructor/builder param), or
   - accept that this is only exercised indirectly and instead unit-test
     that the built `reqwest::Client` config has non-default timeouts set
     (reqwest doesn't expose getters for this though, so the constructor
     injection approach is preferred).

   Simplest approach: add a private `with_transcriber_and_timeouts` (or
   thread `connect_timeout`/`timeout` through `WhisperTranscribeConfig`)
   so the test can pass e.g. 200ms and confirm a slow-drip server triggers
   `WhisperTranscribeError::Download` rather than hanging the test suite.

## Verification

- `cargo test -p brain3 --no-run`
- `cargo test` (targeted: `cargo test -p brain3-platform whisper_transcribe`
  or whatever the actual crate/module path resolves to)
- Re-read the diff to confirm `list_schemas()` still omits `_meta` for tools
  that don't declare it (no behavior change for existing native tools).
