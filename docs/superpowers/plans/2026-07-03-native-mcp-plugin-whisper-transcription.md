Plan: Native MCP Plugin System + Whisper Transcription (first plugin)

### Background

The `audio_mcp_experiment` branch added `save_audio_file` as a container-side MCP tool (Python/FastMCP, in `brain3-mcp-vault-tools`). It downloads an OpenAI file reference (`download_url`) and writes it to a temp dir inside the container. That worked for downloading, but transcription needs `whisper-rs` (a `whisper.cpp` binding), and running ML inference inside the Linux container is undesirable:

- The container is a Linux sandbox (Docker on Linux, `macos-container`/Docker on macOS) with no GPU/Metal/CoreML access, no AVX-tuned build for the host CPU, and today runs on a locked-down internal-only network (`B3_CONTAINER_INTERNAL_NETWORK_ISOLATION`) — a poor place to also start pulling multi-hundred-MB model files from the internet.
- We don't want to bloat the container image with ML deps/models for a feature most users may not enable.

So we need a way to run certain MCP tools **natively in the gateway process** (host Rust binary, not the container), while the container remains the default home for everything else. This is the first of what will eventually be several native tools, so the plugin surface should be intentionally minimal but not a dead end.

**Important constraint discovered while reviewing the code:** the container filesystem is *not* shared with the host. The only bind mount today is the vault path (`B3_VAULT_PATH` → `/vault`; see `crates/platform/src/container/startup.rs:152,168`). That means a file downloaded *inside the container* is invisible to the host-native gateway process. So the new tool does its own download directly in the gateway process (which already makes outbound HTTPS calls for OAuth/tunnel setup, so it has network egress) — same request-download approach as the experimental `save_audio_file`, just running in Rust on the host end-to-end. **`save_audio_file` is retired** — the container never had it merged to `main`, so there's no back-compat concern.

### Decisions (confirmed)

- **One new tool only: `transcribe_audio_file`.** No coexistence with `save_audio_file`, no separate save-then-transcribe split. Single `tools/call` in, transcript out.
- **Default model: `ggml-base.en.bin`.** This is a stock [whisper.cpp](https://github.com/ggerganov/whisper.cpp) ggml-format model — the `ggml-` prefix and `.en` suffix (English-only variant) are whisper.cpp's own naming convention for the quantized models it publishes on Hugging Face, not something we're inventing. `tiny.en` is offered as a faster/lower-quality option in the setup picker.
- **English-only for now.** Only `.en` model variants are offered; no multilingual option in the picker yet. Revisit if non-English users ask.
- **No `download_url` host allowlist.** Too many valid client-specific hosts to enumerate reliably. Instead, guard on content: validate the downloaded bytes actually decode as audio (via the decode step itself — if `symphonia` can't parse it, reject) and enforce the size cap below. This is a materially weaker mitigation than a host allowlist for SSRF — see Security section, it needs to be called out in the threat model as an accepted gap, not silently dropped.
- **Max audio file size: configurable, default 50 MB.** New config value (see below).
- **Platform scope for v1: macOS only, with GPU acceleration (Metal) enabled.** Linux/Windows native-tool support is out of scope for this first cut — see phased plan and doc updates below. This pairs naturally with GPU acceleration since Metal is the macOS-only acceleration path anyway.
- **Naming:** the port/registry are referred to as "NativeMCP" conversationally, but the existing codebase convention for this exact abbreviation is `Mcp`, not `MCP` (see `McpProxyPort`, `McpProxyRequest`, `McpProxyResponse` in `crates/core/src/ports/mcp_proxy.rs`). To stay consistent, the actual Rust identifiers are `NativeMcp` (trait) and `NativeMcpRegistry` (registry struct) — same concept, codebase-conventional casing.

---

### Goals

- A native MCP tool, `transcribe_audio_file`, that runs entirely inside the gateway process (`apps/gateway`), takes an OpenAI file reference as input, downloads the audio, transcribes it with `whisper-rs` (Metal-accelerated on macOS), and returns the transcript text — synchronously, one MCP `tools/call` in, one result out.
- A minimal **native tool registry** (`NativeMcpRegistry`) in the gateway that can register 1..N native tools later without redesign, even though today there is exactly one (`transcribe_audio_file`).
- A single setup-wizard flag, enabled by default, to turn native audio transcription on/off (macOS only for now), plus a model picker (`tiny.en` / `base.en`, more `.en` sizes later) that downloads the chosen ggml model on demand into `~/.brain3/whisper-models/`.
- Threat model updated for the two new capabilities this introduces: gateway-initiated internet egress to fetch audio + models, and gateway-side parsing/execution of untrusted binary audio content (previously that always happened inside the sandboxed container, never in the trusted host process).

### Non-goals (for this plan)

- No generic "run arbitrary native code" plugin marketplace. One hardcoded plugin (`transcribe_audio_file`), wired in explicitly.
- No async/background job polling for MCP (`tools/call` stays request/response). Long transcriptions just make the caller wait.
- No Linux/Windows support for native tools in this pass (see open question resolved above — macOS only for v1).
- No multilingual model support in this pass (English-only `.en` models only).

---

### Architecture: gateway becomes a selective MCP router, not a pure byte proxy

Today `ProxyMcpUseCase` (`crates/core/src/application/proxy_mcp.rs`) is a dumb forwarder: every request's raw bytes go to the container, the response's raw bytes come back. The one exception is `log_save_audio_file_request_summary`, which already **peeks** at the JSON-RPC body to log `tools/call` details without altering routing — proof that body inspection at this layer is an established pattern, just not yet used for routing decisions. (That logging helper goes away along with `save_audio_file`.)

We extend this into real routing:

1. **`initialize`, `resources/*`, `prompts/*`, `ping`, everything else** → forwarded to the container unchanged, exactly as today.
2. **`tools/list`** → forwarded to the container, response parsed, and any enabled native tool's schema is appended to the `tools` array before returning to the client.
3. **`tools/call`** → if `params.name` matches an enabled native tool, do **not** contact the container at all; dispatch to `NativeMcpRegistry` and synthesize the JSON-RPC response locally. Otherwise forward as today.

New pieces:

- **Port** — `crates/core/src/ports/native_mcp.rs`:
  ```rust
  #[async_trait]
  pub trait NativeMcp: Send + Sync {
      fn name(&self) -> &str;
      fn description(&self) -> &str;
      fn input_schema(&self) -> serde_json::Value; // JSON Schema, embedded in tools/list
      async fn call(&self, arguments: serde_json::Value) -> Result<NativeMcpOutput, NativeMcpError>;
  }
  ```
  `NativeMcpOutput` maps directly to an MCP `CallToolResult` (text content block(s) + `isError`).

- **Registry** — `crates/core/src/application/native_mcp_registry.rs`: `NativeMcpRegistry { tools: Vec<Arc<dyn NativeMcp>> }` with `find(name)`, `list_schemas()`. Built once at gateway startup from config (which plugins are enabled) — for now, zero or one entries.

- **Router use case** — new `McpRouterUseCase` (or extend `ProxyMcpUseCase`) in `crates/core/src/application/`, wrapping the existing proxy + `NativeMcpRegistry`, implementing the three-way routing above. `apps/gateway/src/server.rs` wires this in in place of calling `ProxyMcpUseCase` directly.

- **Adapter (the plugin itself)** — `crates/platform/src/native_mcp/whisper_transcribe.rs`, implementing `NativeMcp`:
  - Input: same shape as Python's `OpenAIFileReferenceInput` (`download_url`, `file_id`, `mime_type`, `file_name`) — a Rust struct with `serde`.
  - `call()`:
    1. Stream-download to a temp file, enforcing the configured max-size cap (default 50 MB) via a running byte counter — abort the download the moment it's exceeded, don't buffer the whole thing first.
    2. Decode/resample audio to 16 kHz mono `f32` PCM (needed by `whisper-rs`) using `symphonia`. A decode failure is treated as "not a valid audio file" and returned as a tool error — this is the file-type guard in place of a host allowlist.
    3. Run whisper inference via `spawn_blocking` (CPU/GPU-bound work must not block the tokio runtime, even though the MCP call itself stays synchronous from the client's point of view).
    4. Return transcript text as the tool result; delete the temp file.
  - Model loaded lazily once per gateway process and cached in memory (loading a ggml model per call would be slow); model path comes from config.

- **Dependency**: add `whisper-rs` (with the `metal` feature enabled on macOS builds) and `symphonia` to `crates/platform/Cargo.toml`. `whisper-rs` builds/links `whisper.cpp` (C++) via its `-sys` crate — this adds a C++ toolchain requirement to the gateway build. Given v1 is macOS-only for this feature, gate the dependency and the `native_mcp` module behind `#[cfg(target_os = "macos")]` (or a Cargo feature flag mirrored by target) so Linux/Windows builds are entirely unaffected and don't need a C++ toolchain at all.

---

### Model management

- Storage: `$B3_HOME/whisper-models/` (i.e. `~/.brain3/whisper-models/` via the existing `Brain3AppHome`, same place `.env`/`cloudflared`/`brain3.db` already live).
- Models are the standard ggml quantized `.en` files whisper.cpp publishes (`ggml-tiny.en.bin`, `ggml-base.en.bin`, ...).
- **On-demand download**: during setup, once the user opts into native audio transcription, the wizard presents a model choice (`tiny.en`, `base.en` default) and downloads the corresponding `.bin` file into that directory, showing progress. If the file already exists (re-running setup, or switching back to a previously-downloaded model), skip re-downloading.
- **Integrity check required**: verify a SHA256 checksum against a hardcoded known-good table per model (whisper.cpp's own download script publishes these) before treating the file as usable. This is a supply-chain surface (fetching a binary blob that then gets loaded by a C++ inference engine) and deserves the same scrutiny as any other dependency fetch.
- Config:
  - `B3_NATIVE_AUDIO_TRANSCRIPTION_ENABLED` (bool, default `true`, macOS only — see below)
  - `B3_WHISPER_MODEL` (e.g. `base.en`)
  - `B3_WHISPER_MAX_AUDIO_BYTES` (integer, default `52428800` i.e. 50 MB)
  - all read via `crates/platform/src/config`.
- On non-macOS platforms, this flag should not be offered / should be forced off, since the underlying feature isn't built there in v1 (see Doc updates below).

### Setup wizard integration

- New screen in `apps/gateway/src/tui/screens.rs` (pattern-matching existing screens), shown after vault/container setup, **gated on `cfg!(target_os = "macos")`**:
  - "Enable native audio transcription tools? (Y/n)" — default yes, only shown on macOS; skipped entirely on Linux/Windows for v1.
  - If yes: "Choose a Whisper model:" list (`tiny.en`, `base.en` default), then download with a progress indicator and checksum verification.
- Re-running `brain3 --setup` should let the user change the model (re-download) or disable the feature, not just set it once.

---

### Security / threat model updates (required before merging — SECURITY_AUDIT.MD § Threat Model)

This is the part I'd push back on hardest if I were your intern, so flagging it clearly even though you've made the calls:

1. **New trust boundary crossed.** Every byte of untrusted audio content, and the third-party libraries that parse it (`symphonia`, `whisper-rs`/`whisper.cpp`), now execute **inside the gateway process** — the same process holding OAuth secrets, the token DB, and the upstream shared secret. Today, anything that parses attacker-influenced binary data happens inside the sandboxed, network-isolated container. A memory-safety bug in `whisper.cpp` (C++) or a decoder bug in `symphonia` is now a bug in your most privileged component, not a disposable container. Accepted tradeoff per your call — document it as such.
2. **No host allowlist on `download_url` = accepted SSRF-adjacent gap.** Since we're not validating the download host, `transcribe_audio_file` will fetch whatever URL it's given, including (in principle) internal/loopback addresses if a malicious or buggy client supplied one — the container's internal-network isolation doesn't protect the gateway process itself. The content-type guard (must decode as valid audio) narrows the blast radius (an attacker can't use this to read arbitrary URL contents back verbatim, only to get "is this parseable as audio" oracle behavior, or to make the gateway issue a GET to an internal URL as a side effect) but does not fully close SSRF. Document this explicitly in the threat model as accepted risk, not an oversight.
3. **New egress path for model downloads.** The gateway now makes outbound HTTPS requests to fetch whisper models (Hugging Face or wherever whisper.cpp sources them from) during setup. Lower risk since it's setup-time, user-initiated, and checksum-verified, but still new egress worth listing.
4. **Resource exhaustion.** A malicious or oversized audio file could tie up a `spawn_blocking` thread for a long time. The 50 MB (configurable) size cap bounds download cost; consider also capping decoded audio duration before running inference (a small file can still decode to a very long recording if bitrate is low) as a cheap follow-up.
5. **Model file integrity** via checksum, per above — treat it like any other fetched-then-executed artifact.

Per AGENTS.MD, these need to be written into `SECURITY_AUDIT.MD` under **Threat Model** (Assets / Trust Boundaries / Attacker Capabilities sections) before this ships, not after.

---

### Documentation updates required (macOS-only v1)

Since this feature only works on macOS for now, and AGENTS.MD's "Updating new release" section names exactly which files get touched per release, make sure the following are updated to say so explicitly (not silently omit Linux/Windows):

- `README.md` — document `transcribe_audio_file`, note it's macOS-only for now (native, Metal-accelerated, no container involvement), and that Linux/Windows support is planned but not yet available.
- Setup wizard screen text itself (shown only on macOS, per above) should say why it's absent elsewhere if a user asks — consider a one-line note in the Linux/Windows setup flow ("native audio transcription: not yet available on this platform") rather than just silently skipping the screen, so it's discoverable rather than looking like a missing feature.
- `first_run_setup.rs` / `.env.template` — document the three new `B3_*` vars (enabled flag, model, max bytes) with the same comment-style as existing entries, noting the macOS-only constraint on the enabled flag.

---

### Phased implementation

1. Spike: add `whisper-rs` (with `metal` feature) + `symphonia` to `crates/platform`, gated `#[cfg(target_os = "macos")]`, confirm it builds cleanly on macOS CI/local before writing anything else. (Linux/Windows CI should be unaffected since the dependency is cfg-gated out.)
2. Core: add `NativeMcp` port, `NativeMcpRegistry`, and the new router use case (with unit tests against a fake `NativeMcp`, following the existing `CapturingProxy` test pattern in `proxy_mcp.rs`).
3. Platform: implement `WhisperTranscribeTool` (download with size cap → decode via symphonia → resample → Metal-accelerated inference → return), retiring `save_audio_file` and its Python-side code/tests.
4. Config + setup wizard: add the enable flag (macOS-gated), model picker screen (`tiny.en`/`base.en`), on-demand download with checksum verification into `~/.brain3/whisper-models/`.
5. Wire the router into `apps/gateway/src/server.rs` in place of the direct `ProxyMcpUseCase` call.
6. Update `SECURITY_AUDIT.MD` Threat Model section with the new trust boundary, egress paths, and the explicitly-accepted SSRF-adjacent gap.
7. Update `README.md` / `.env.template` / setup wizard copy per the Documentation section above.
8. `cargo test -p brain3 --no-run` + `cargo test`, plus a manual end-to-end check on macOS: enable the flag, download `base.en`, call `transcribe_audio_file` with a real short audio file, confirm transcript quality and that the container is never contacted for this call (check gateway logs).
