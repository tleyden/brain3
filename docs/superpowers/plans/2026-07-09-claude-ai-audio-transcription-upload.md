# Plan: make `transcribe_audio_file` work from Claude.ai, not just ChatGPT

## Verified problem statement

`transcribe_audio_file` (`crates/platform/src/native_mcp_tools/whisper_transcribe.rs`)
only accepts an OpenAI-style file reference:

```json
{ "audio_file": { "download_url": "...", "file_id": "...", ... } }
```

and `meta()` advertises `{"openai/fileParams": ["audio_file"]}` — an OpenAI
Apps SDK convention that tells ChatGPT to auto-bind an uploaded file to the
`audio_file` argument and hand the tool a temporary authorized
`download_url` it can `GET`.

Claude.ai has no equivalent convention. When a user attaches an audio file
in a Claude.ai chat with "Code execution and file creation" enabled, the
bytes land on the code-execution **sandbox's own filesystem**, not behind a
fetchable `download_url`. There is no `_meta` hint Claude.ai honors that
would cause it to synthesize one. So `transcribe_audio_file` is currently
unreachable from Claude.ai — confirmed, not a misconfiguration.

## Correction: what the MCP spec actually says about binary tool *inputs*

A second assistant-generated message claimed "the MCP spec defines
`AudioContent` with base64 `data`... the same pattern applies to tool
inputs." I checked this against the spec directly
(`modelcontextprotocol.io/specification/draft/server/tools` and `.../schema`)
and it's **half right, stated too strongly**:

- `AudioContent`/`ImageContent` (`{ "type": "audio", "data": "<base64>",
  "mimeType": "..." }`) are real, spec-defined types — but they're part of
  `ContentBlock`, used for **tool results** (and prompts/resources), not
  tool call *arguments*.
- Tool **inputs** are governed purely by a server's own `inputSchema` (plain
  JSON Schema). The spec places zero constraints on how a server author
  models a "file" input. A base64 string property is a legal and common
  choice, but it is exactly as much of an "application-level convention" as
  brain3's existing `{ download_url, file_id }` shape (which is OpenAI's
  convention) or the presigned-upload-URL shape (a pattern several MCP
  servers have independently converged on, per the earlier research pass) —
  none of the three is more "spec-native" than the others. The spec is
  silent on file input modeling by design.

This matters for the plan because it means base64-inline is not free of
design trade-offs just because it "is in the spec" — it isn't, in the input
direction. It's still worth strongly considering on its own merits (below),
just not on the basis of spec compliance.

## ⚠️ Non-negotiable constraint: do not touch the working OpenAI path

**The existing `{ download_url, file_id }` shape is live, tested, and
confirmed working end-to-end with ChatGPT today.** Nothing in this plan
should modify its behavior, its wire format, its `_meta` hint, or the
existing passing tests for it
(`whisper_transcribe.rs::call_downloads_decodes_resamples_and_returns_transcript`
and friends). Both options below are strictly *additive*: every code path
that already ships must keep working, byte-for-byte, exactly as it does
today. If an implementation of either option requires changing how the
`download_url`/`file_id` shape is parsed or validated, that's a sign the
approach is wrong — go back and find a purely-additive way instead.

## Revised design: two viable options, staged by effort/risk

### Option A — inline base64 argument (new, recommended to try first)

**This is not a new tool.** It's the *same* `transcribe_audio_file` tool,
same registration, same name, same OpenAI shape untouched — just one more
accepted shape for its existing `audio_file` argument, sitting alongside
`{ download_url, file_id }` as an alternative (e.g. a `oneOf` branch in the
JSON Schema, or simply extra optional fields with server-side logic that
picks a path based on which fields are present). A client sends *either*
the existing OpenAI-style reference *or* this new one — never both, and the
OpenAI path is completely unaffected by this addition:

```json
{ "audio_file": { "audio_data": "<base64>", "mime_type": "audio/wav", "file_name": "memo.wav" } }
```

Server-side: `base64::decode`, then feed straight into the *existing*
`decode_audio_file_to_whisper_pcm` path (same size-limit enforcement, same
whisper-rs inference) — no HTTP GET, no new HTTP route, no new registry, no
new tool.

**Why this is attractive:**
- Zero new ingress. The bytes travel over the *existing* OAuth-protected
  `/mcp` JSON-RPC channel — no new trust boundary, no `SECURITY_AUDIT.md`
  rewrite beyond noting the input variant exists.
- No dependency on Cloudflare tunnel naming, "Additional allowed domains,"
  or the sandbox's `curl` working at all.
- Much smaller code change: one new struct variant + one new schema branch
  in a file that already exists, reusing 100% of the decode/limit/inference
  pipeline.

**Why it's genuinely uncertain whether it works from Claude.ai chat, and
needs to be tested empirically before committing to it as the answer:**
- For Claude.ai to call the tool with `audio_data` populated, it needs to
  get the attached file's raw bytes into that JSON argument somehow. Two
  paths exist, and I could not confirm from docs which (if either) applies
  to the **Claude.ai consumer chat product** (as opposed to the raw
  Developer Platform/Messages API, which is a different product surface):
  1. *Without code execution*: the model would have to literally emit the
     base64 text as tokens in its own response. Per-turn output token caps
     (commonly in the thousands, model/config-dependent) make this
     impractical past a very short clip, and — per the earlier research
     pass — Claude.ai doesn't appear to give the model raw byte access to
     an attached audio file without code execution in the first place.
  2. *With code execution enabled*, Anthropic's documented "[programmatic
     tool calling](https://www.anthropic.com/engineering/code-execution-with-mcp)"
     pattern lets sandboxed code call an MCP tool directly — the tool
     argument (including a base64 blob it constructs from a sandbox-local
     file) is assembled and sent by the *script*, not typed out by the
     model, so it doesn't consume output tokens or context. This is real
     and documented for the API, but I found no confirmation it's exposed
     in Claude.ai's consumer chat UI today rather than being an
     API/agent-harness-only capability.
- Practical size ceiling either way is much smaller than the tool's current
  50MB (`max_audio_bytes`) download cap — a 50MB file would be ~67MB of
  base64 text, unworkable as a JSON-RPC argument or (if per case 1 above)
  model output. Recommend a separate, much smaller cap specifically for the
  inline-base64 path (e.g. single-digit MB of raw audio — enough for a
  multi-minute voice memo, not a long recording) rather than reusing
  `max_audio_bytes` as-is.

**Verification step before writing any code**: attach a short (~10-30s)
audio clip in an actual Claude.ai chat with code execution + this gateway's
MCP connector enabled, and ask Claude to transcribe it via a temporary test
tool that just echoes back `len(audio_data)`. This directly answers "can
Claude.ai get sandbox-local bytes into a tool argument at all" before
investing in either Option A's real implementation or Option B below.

### Option B — presigned-upload-URL + sandbox `curl` (fallback, kept from prior plan draft, demoted not dropped)

If Option A's verification step shows Claude.ai chat cannot practically get
file bytes into a tool argument (no programmatic-tool-calling access, or
sizes that matter for real recordings blow past output-token limits), fall
back to the previously researched, independently-confirmed pattern: a
`request_audio_upload` tool hands back a short-TTL single-use presigned
`PUT` URL, Claude's sandbox `curl`s the file to it directly (bypassing the
model's context entirely), then `transcribe_audio_file` is called with an
`upload_id`.

This option is real (confirmed against official docs, the Claude Help
Center, and an independent third-party MCP server doing exactly this) but
costs meaningfully more:
- A genuinely new, only-token-gated (not bearer-gated) public HTTP ingress
  point — requires a `SECURITY_AUDIT.md` Threat Model update per AGENTS.MD
  before/with implementation (new asset: in-flight uploaded bytes; new
  trust boundary: possession of an unguessable single-use 5-minute token
  substitutes for the OAuth bearer token on this one route; new attacker
  capability: anyone who can reach the public origin and guess/observe a
  live token within its TTL can PUT bytes that get whisper-decoded,
  mitigated by the same size cap + decode validation already in place).
- Operational dependency: requires a **named/persistent** Cloudflare
  tunnel, since Claude.ai's "Additional allowed domains" allowlist is
  hostname-keyed and the default quick tunnel's URL rotates on every
  gateway restart (SECURITY_AUDIT.md Finding [3]).
- More moving parts: new upload-token registry with TTL/single-use/cleanup
  semantics, a new HTTP handler, a new `NativeMcpTool`.

Design details (registry shape, route placement, size-cap-while-streaming,
reuse of `max_audio_bytes`, test list) are unchanged from the prior version
of this plan and are kept below in case Option A doesn't pan out.

<details>
<summary>Option B implementation detail (collapsed — only needed if Option A fails verification)</summary>

1. Add an in-memory upload registry (`Mutex<HashMap<upload_id, Entry>>` or
   similar), `Entry { expires_at, used: bool, path: Option<PathBuf> }`, with
   lazy expiry-sweep on access, and cleanup of orphaned temp files for
   uploads that were reserved but never PUT to or never transcribed.
2. Add `request_audio_upload` `NativeMcpTool` impl (mirrors
   `WhisperTranscribeTool`'s shape), registered alongside it in
   `native_mcp_tools_from_config` (`apps/gateway/src/server.rs`). Returns
   the `upload_id`, the `curl -X PUT` command, TTL, and the size cap
   (sourced from the same `max_audio_bytes` config already used by
   `transcribe_audio_file` — no second, divergent limit).
3. Add `PUT /uploads/{upload_id}` handler in `crates/platform/src/http/`
   (new `uploads.rs`), wired into `router.rs`'s `build_router` (must be
   reachable through the public tunnel-facing router, same as `/oauth/*`).
   Streams the body to a temp file with the same byte-cap-while-streaming
   behavior already used in `download_to_temp_file`, rejecting early (413)
   rather than buffering unbounded bytes.
4. Extend `WhisperToolArguments` deserialization to accept the `upload_id`
   variant, sharing the existing decode/size-limit/inference path.
5. Tests: upload happy path, size-limit rejection while streaming,
   unknown/expired upload_id, single-use enforcement (second PUT and second
   transcribe both rejected after first success), `transcribe_audio_file`
   accepting `upload_id` end-to-end with a fake transcriber.

</details>

No changes needed in `brain3-mcp-vault-tools` (Python container) under
either option — transcription is a native in-gateway Rust tool, unrelated
to that codebase.

## Wording fix: stop branding the existing shape as OpenAI-only

Today's `input_schema()` describes the `audio_file` object as *"OpenAI file
reference for the uploaded audio file"* and `file_id` as *"OpenAI file
identifier."* That's misleading in the same direction as the base64 claim
above: the `{ download_url, file_id }` **shape** is generic JSON Schema —
any MCP client capable of handing the tool a temporary authorized download
URL could use it. What's actually OpenAI-specific is only the `_meta`
`openai/fileParams` hint that tells *ChatGPT specifically* to populate it
automatically.

Reword (implementation detail, to land alongside whichever option ships):

- `audio_file` description: from *"OpenAI file reference for the uploaded
  audio file"* to something like *"Reference to an uploaded audio file
  accessible via a temporary authorized download URL. Populated
  automatically by OpenAI Apps SDK clients via the `openai/fileParams`
  hint; any MCP client that can supply a fetchable `download_url` may use
  this shape."*
- `file_id` description: from *"OpenAI file identifier"* to *"Client-issued
  identifier for the uploaded file (opaque to this tool; used for
  logging/correlation only)."*

This is purely descriptive-string wording — no behavior change, low risk,
worth doing regardless of which option (A or B) also ships, since it
currently mis-documents the tool for any future non-OpenAI client author
(including whichever shape we add for Claude.ai).

## Implementation order

1. Reword the existing `audio_file`/`file_id` schema descriptions (above) —
   independent, no-risk, do any time.
2. Run the empirical verification step under Option A (attach a short clip
   in a real Claude.ai chat, confirm whether bytes reach a tool argument at
   all, and by which mechanism).
3. If verification succeeds: implement Option A (new `audio_data`/
   `mime_type` schema branch + decode path + a separate, smaller size cap
   for this path + tests) as a **strictly additive** change — do not modify
   `OpenAIFileReferenceInput`, the `download_url`/`file_id` required fields,
   or the existing `_meta` hint. Run the full existing
   `whisper_transcribe.rs` test suite before and after and confirm every
   existing test still passes unmodified — that's the regression signal
   that the working ChatGPT path is intact. No `SECURITY_AUDIT.md` change
   expected beyond a one-line note that this input variant exists, since no
   new trust boundary is created.
4. If verification fails or sizes are impractical: implement Option B per
   the collapsed detail above, including the required `SECURITY_AUDIT.md`
   Threat Model update *before* the new route ships, and confirm with you
   first that you're willing to move to a named Cloudflare tunnel (needed
   for "Additional allowed domains" to survive restarts) and that you're OK
   with a token-gated-rather-than-bearer-gated upload route.
5. Local verification per AGENTS.MD either way: `cargo test -p brain3
   --no-run`, then `cargo test`. If Option B ships (new gateway HTTP
   surface), also run `uv run scripts/e2e_smoke.py` per AGENTS.MD's
   explicit call-out for gateway/proxy changes.

## Open questions for you before I implement anything

- OK with doing the manual Claude.ai verification step (step 2) before
  committing to either option's full implementation? It's the cheapest way
  to avoid building the wrong one.
- If it comes to Option B: confirm you're on an individual Claude.ai plan
  (Free/Pro/Max) — a filed GitHub issue (`anthropics/claude-code#63182`)
  reports the Team-plan org-level allowlist not reaching the sandbox proxy,
  so Option B would be unreliable on Team.
- If it comes to Option B: confirm you're willing to move to a named
  Cloudflare tunnel, and sign off on the token-gated (not bearer-gated)
  upload endpoint as a new ingress shape, given this repo's security
  posture.
