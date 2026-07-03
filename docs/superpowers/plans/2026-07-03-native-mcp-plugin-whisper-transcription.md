Plan: Native MCP Plugin System + Whisper Transcription (first plugin)

### Background

The `audio_mcp_experiment` branch added `save_audio_file` as a container-side MCP tool (Python/FastMCP, in `brain3-mcp-vault-tools`). It downloads an OpenAI file reference (`download_url`) and writes it to a temp dir inside the container. That worked for downloading, but transcription needs `whisper-rs` (a `whisper.cpp` binding), and running ML inference inside the Linux container is undesirable:

- The container is a Linux sandbox (Docker on Linux, `macos-container`/Docker on macOS) with no GPU/Metal/CoreML access, no AVX-tuned build for the host CPU, and today runs on a locked-down internal-only network (`B3_CONTAINER_INTERNAL_NETWORK_ISOLATION`) — a poor place to also start pulling multi-hundred-MB model files from the internet.
- We don't want to bloat the container image with ML deps/models for a feature most users may not enable.

So we need a way to run certain MCP tools **natively in the gateway process** (host Rust binary, not the container), while the container remains the default home for everything else. This is the first of what will eventually be several native tools, so the plugin surface should be intentionally minimal but not a dead end.

**Important constraint discovered while reviewing the code:** the container filesystem is *not* shared with the host. The only bind mount today is the vault path (`B3_VAULT_PATH` → `/vault`; see `crates/platform/src/container/startup.rs:152,168`). That means a file `save_audio_file` downloads and writes *inside the container* is invisible to the host-native gateway process. So splitting the work as "container downloads, host transcribes" doesn't work without adding a shared volume. The clean fix is to stop downloading in the container at all: the native tool does the download itself (gateway already makes outbound HTTPS calls for OAuth/tunnel setup, so it has network egress) and `save_audio_file` (container-side) is retired once the native tool ships.

### Goals

- A native MCP tool, `transcribe_audio`, that runs entirely inside the gateway process (`apps/gateway`), takes an OpenAI file reference as input, downloads the audio, transcribes it with `whisper-rs`, and returns the transcript text — synchronously, one MCP `tools/call` in, one result out.
- A minimal **native tool registry** in the gateway that can register 1..N native tools later without redesign, even though today there is exactly one.
- A single setup-wizard flag, enabled by default, to turn native audio transcription on/off, plus a model picker that downloads the chosen ggml model on demand into `~/.brain3/whisper-models/`.
- Threat model updated for the two new capabilities this introduces: gateway-initiated internet egress to fetch models, and gateway-side parsing/execution of untrusted binary audio content (previously that always happened inside the sandboxed container, never in the trusted host process).

### Non-goals (for this plan)

- No generic "run arbitrary native code" plugin marketplace. One hardcoded plugin (`whisper_transcribe`), wired in explicitly — same spirit as "not building a plugin system" but leaving a clean seam.
- No async/background job polling for MCP (`tools/call` stays request/response). Long transcriptions just make the caller wait.
- No change to the existing container-side tools other than removing/retiring `save_audio_file`.

---

### Architecture: gateway becomes a selective MCP router, not a pure byte proxy

Today `ProxyMcpUseCase` (`crates/core/src/application/proxy_mcp.rs`) is a dumb forwarder: every request's raw bytes go to the container, the response's raw bytes come back. The one exception is `log_save_audio_file_request_summary`, which already **peeks** at the JSON-RPC body to log `tools/call` details without altering routing — proof that body inspection at this layer is an established pattern, just not yet used for routing decisions.

We extend this into real routing:

1. **`initialize`, `resources/*`, `prompts/*`, `ping`, everything else** → forwarded to the container unchanged, exactly as today.
2. **`tools/list`** → forwarded to the container, response parsed, and any enabled native tool's schema is appended to the `tools` array before returning to the client.
3. **`tools/call`** → if `params.name` matches an enabled native tool, do **not** contact the container at all; dispatch to the native tool registry and synthesize the JSON-RPC response locally. Otherwise forward as today.

New pieces:

- **Port** — `crates/core/src/ports/native_tool.rs`:
  ```rust
  #[async_trait]
  pub trait NativeTool: Send + Sync {
      fn name(&self) -> &str;
      fn description(&self) -> &str;
      fn input_schema(&self) -> serde_json::Value; // JSON Schema, embedded in tools/list
      async fn call(&self, arguments: serde_json::Value) -> Result<NativeToolOutput, NativeToolError>;
  }
  ```
  `NativeToolOutput` maps directly to an MCP `CallToolResult` (text content block(s) + `isError`).

- **Registry** — `crates/core/src/application/native_tool_registry.rs`: `NativeToolRegistry { tools: Vec<Arc<dyn NativeTool>> }` with `find(name)`, `list_schemas()`. Built once at gateway startup from config (which plugins are enabled) — for now, zero or one entries.

- **Router use case** — new `McpRouterUseCase` (or extend `ProxyMcpUseCase`) in `crates/core/src/application/`, wrapping the existing proxy + the registry, implementing the three-way routing above. `apps/gateway/src/server.rs` wires this in in place of calling `ProxyMcpUseCase` directly.

- **Adapter (the plugin itself)** — `crates/platform/src/native_tools/whisper_transcribe.rs`, implementing `NativeTool`:
  - Input: reuse the same shape as Python's `OpenAIFileReferenceInput` (`download_url`, `file_id`, `mime_type`, `file_name`) — a Rust struct with `serde`.
  - `call()`:
    1. Validate `download_url` host against an allowlist (see Security below).
    2. Stream-download to a temp file, capped at a max byte size.
    3. Decode/resample audio to 16 kHz mono `f32` PCM (needed by `whisper-rs`) — use `symphonia` for container/codec decoding.
    4. Run whisper inference via `spawn_blocking` (CPU/GPU-bound work must not block the tokio runtime, even though the MCP call itself stays synchronous from the client's point of view).
    5. Return transcript text as the tool result; delete the temp file.
  - Model loaded lazily once per gateway process and cached in memory (loading a ggml model per call would be slow); model path comes from config.

- **Dependency**: add `whisper-rs` (and `symphonia` for decoding) to `crates/platform/Cargo.toml`. `whisper-rs` builds/links `whisper.cpp` (C++) via its `-sys` crate — this adds a C++ toolchain requirement to the gateway build; worth confirming CI images (Linux + macOS + the Windows distribution mentioned in AGENTS.MD) all have a working C++ compiler before committing to this.

---

### Model management

- Storage: `$B3_HOME/whisper-models/` (i.e. `~/.brain3/whisper-models/` via the existing `Brain3AppHome`, same place `.env`/`cloudflared`/`brain3.db` already live).
- Models are the standard ggml quantized files whisper.cpp publishes (tiny/base/small/medium/large, `.en`-only or multilingual variants).
- **On-demand download**: during setup, once the user opts into native audio transcription, the wizard presents a model choice and downloads the corresponding `.bin` file into that directory, showing progress. If the file already exists (re-running setup, or switching back to a previously-downloaded model), skip re-downloading.
- **Integrity check required**: verify a SHA256 checksum against a hardcoded known-good table per model (whisper.cpp's download script publishes these) before treating the file as usable. This is a new supply-chain surface (fetching a binary blob that then gets loaded by a C++ inference engine) and deserves the same scrutiny as any other dependency fetch.
- Config: `B3_NATIVE_AUDIO_TRANSCRIPTION_ENABLED` (bool, default `true` per your ask) and `B3_WHISPER_MODEL` (e.g. `base.en`) in `.env` / `.env.template`, read via `crates/platform/src/config`.

### Setup wizard integration

- New screen in `apps/gateway/src/tui/screens.rs` (pattern-matching existing screens), shown after vault/container setup:
  - "Enable native audio transcription tools? (Y/n)" — default yes.
  - If yes: "Choose a Whisper model:" list (tiny.en, base.en, small.en, ...), default to a small/fast one (see open question below), then download with a progress indicator.
- Re-running `brain3 --setup` should let the user change the model (re-download) or disable the feature, not just set it once.

---

### Security / threat model updates (required before merging — SECURITY_AUDIT.MD § Threat Model)

This is the part I'd push back on hardest if I were your intern, so flagging it clearly:

1. **New trust boundary crossed.** Every byte of untrusted audio content, and the third-party libraries that parse it (`symphonia`, `whisper-rs`/`whisper.cpp`), now execute **inside the gateway process** — the same process holding OAuth secrets, the token DB, and the upstream shared secret. Today, anything that parses attacker-influenced binary data (file downloads, etc.) happens inside the sandboxed, network-isolated container. A memory-safety bug in `whisper.cpp` (C++, historically has had such CVEs in ggml-based projects) or a decoder bug in `symphonia` is now a bug in your most privileged component, not a disposable container. This should be called out explicitly as an accepted tradeoff, not something that slips in as an implementation detail.
2. **New egress path.** The gateway now makes outbound HTTPS requests to (a) wherever the OpenAI `download_url` points, and (b) wherever whisper models are fetched from (e.g. Hugging Face). `download_url` values come from the MCP client / caller — validate the host against an allowlist of expected OpenAI file-serving domains before fetching, to avoid turning `transcribe_audio` into an SSRF primitive against your own container's internal network or localhost-bound services (recall the container network is otherwise locked down — don't undo that by proxying arbitrary URLs through the gateway).
3. **Resource exhaustion.** A malicious or oversized audio file could tie up a `spawn_blocking` thread for a long time. Enforce a max download size and (if feasible) a max audio duration before running inference, and document the limit in the tool's description/error so the calling AI can react sensibly.
4. **Model file integrity**, per above — treat it like any other fetched-then-executed artifact.

Per AGENTS.MD, these need to be written into `SECURITY_AUDIT.MD` under **Threat Model** (Assets / Trust Boundaries / Attacker Capabilities sections) before this ships, not after.

---

### Things I need you to decide (challenging the plan where it's underspecified)

1. **Does `transcribe_audio` replace `save_audio_file`, or coexist?** Given the container/host filesystem split above, keeping `save_audio_file` around doesn't accomplish anything for the transcription flow — I'd retire it and have `transcribe_audio` do download+transcribe in one call. Confirm you're fine dropping `save_audio_file` entirely (it's only on the experimental branch, not merged to main, so no back-compat concern).
2. **Default model.** You said "small default model" — I'd propose `ggml-base.en.bin` (~148 MB, English-only, fast on CPU) as the out-of-the-box default, with `tiny.en` as the "fastest but least accurate" option and multilingual variants available in the picker for non-English users. Confirm, or pick a different default.
3. **Non-English audio.** `.en`-only models are smaller/faster but silently produce garbage on non-English input. Do we default to an `.en` model and accept that non-English users must explicitly pick a multilingual model during setup, or default to multilingual (`base`, no `.en`) to avoid surprises? I'd lean "default `.en`, let the tool's error/description mention multilingual models exist" — but this is a real UX tradeoff worth your call.
4. **`download_url` host allowlist.** What hosts are actually valid here — is this exclusively OpenAI's file-serving domain(s), or could Claude/other clients pass a different host? I don't have visibility into what `download_url` looks like from a Claude-originated call vs a ChatGPT one; needs a concrete allowlist before this ships, not a TODO.
5. **Max file size / duration limits** — what's acceptable? E.g. reject audio > 25 MB or > 30 minutes, matching typical voice-memo use cases? Need actual numbers, not "reasonable".
6. **Build/CI impact.** `whisper-rs` requires a C++ toolchain to build (links `whisper.cpp`). Given AGENTS.MD explicitly calls out Windows distribution as a target, I'd want to verify the Windows build (and CI runners for all three platforms) can compile this before committing — this could be a bigger lift than the Rust code itself. Worth a spike before writing the full implementation.
7. **Optional GPU acceleration.** `whisper-rs` supports Metal (macOS)/CUDA feature flags. Do we enable Metal on macOS builds for this first cut, or keep it CPU-only everywhere initially and revisit? CPU-only is simpler to ship and test consistently across platforms; I'd suggest starting there and treating acceleration as a fast-follow.

---

### Phased implementation (once the above is settled)

1. Spike: add `whisper-rs` + `symphonia` to `crates/platform`, confirm it builds on macOS, Linux, and Windows CI. This de-risks item 6 above before anything else is built.
2. Core: add `NativeTool` port, `NativeToolRegistry`, and the new router use case (with unit tests against a fake `NativeTool`, following the existing `CapturingProxy` test pattern in `proxy_mcp.rs`).
3. Platform: implement `WhisperTranscribeTool` (download → decode → resample → infer → return), with the host allowlist and size/duration limits from the open questions above.
4. Config + setup wizard: add the enable flag, model picker screen, on-demand download with checksum verification into `~/.brain3/whisper-models/`.
5. Wire the router into `apps/gateway/src/server.rs` in place of the direct `ProxyMcpUseCase` call.
6. Retire `save_audio_file` from `brain3-mcp-vault-tools` (container side) once `transcribe_audio` covers the same use case.
7. Update `SECURITY_AUDIT.MD` Threat Model section with the new trust boundary, egress paths, and mitigations.
8. `cargo test -p brain3 --no-run` + `cargo test`, plus a manual end-to-end check: enable the flag, download a model, call `transcribe_audio` with a real short audio file, confirm transcript quality and that the container is never contacted for this call (check gateway logs).
