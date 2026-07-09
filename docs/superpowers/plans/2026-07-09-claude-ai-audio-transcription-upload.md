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

## Verified: is the proposed presigned-upload-URL pattern real?

Yes, cross-checked against three independent sources (not just the
assistant-generated script, which I don't treat as authoritative on its
own):

- **Official docs** (`platform.claude.com/docs/.../code-execution-tool`):
  confirms user file uploads made through the **raw Messages API** use a
  `container_upload` block + `file_id` from the Files API — that's the
  API-level mechanism and it's a red herring for Claude.ai chat.
- **Claude Help Center** ("Create and edit files with Claude"): confirms
  Settings → Capabilities → Code execution and file creation has an
  "Additional allowed domains" network allowlist, and that this exists for
  **individual Free/Pro/Max accounts**, not just Team/Enterprise. Also
  confirms: *"If MCP integrations are enabled, network communication
  remains possible through those connections regardless of the network
  egress setting"* — i.e. the egress toggle only gates code running inside
  the sandbox (a `curl` call), not MCP tool calls themselves.
- **Independent third-party implementation** (futuresearch.ai blog, "How to
  Upload Large Files to an MCP Server Without Filling the Context Window")
  describes the exact same three-step flow already in production: request a
  presigned URL from an MCP tool → Claude runs `curl -X PUT` inside the
  sandbox → a second MCP tool call exchanges an opaque ID for the parsed
  result. Confirms presigned URLs need a short TTL (theirs: 5 min) and that
  omitting the domain from "Additional allowed domains" fails the `curl`
  with a network error even though the URL is valid.

One caveat found and worth noting, not blocking: a filed GitHub issue
(`anthropics/claude-code#63182`) reports the **Team-plan org-level**
allowlist not reaching the sandbox proxy. Doesn't affect individual
accounts, which is presumably how this user runs Claude.ai.

**Conclusion: the pattern is real and is the documented way to get sandbox-local
bytes into an MCP server. The assistant-generated script is not usable
as-is** — it's a standalone Python/FastMCP/faster-whisper server with no
auth, no framework fit, and it duplicates whisper-rs inference brain3 already
has natively in Rust. It also introduces an unauthenticated public HTTP
upload endpoint, which conflicts directly with this repo's security rule
("gateway is intentionally closed to preregistered clients only") and the
AGENTS.MD rule that new ingress points require a `SECURITY_AUDIT.md` threat
model update first.

## Design goal

Reuse brain3's existing native Rust transcription path (`whisper-rs`,
existing `max_audio_bytes` cap, existing audio decode/size-limit logic) and
add a second, Claude.ai-shaped way to *get bytes into* that path, without
weakening the "only the preregistered client reaches protected data"
guarantee.

Two new pieces, one path reused:

1. **New native MCP tool `request_audio_upload`** (mirrors
   `WhisperTranscribeTool`'s shape in
   `crates/platform/src/native_mcp_tools/`). Only reachable the same way
   every other tool is: through the OAuth-protected `/mcp` route, so only the
   preregistered client can ever obtain an upload token. Returns:
   - a short-TTL (5 min, matching the pattern found in research), single-use,
     cryptographically random `upload_id`
   - the `curl -X PUT` command string Claude should run in its sandbox
   - the size cap, sourced from the *same* `max_audio_bytes` config
     (`config.native_audio_transcription.max_audio_bytes`) already used by
     `transcribe_audio_file` — no second, divergent limit.

2. **New unauthenticated-but-capability-scoped HTTP route**
   `PUT /uploads/{upload_id}` in `crates/platform/src/http/` (new
   `uploads.rs` handler, wired into both `router.rs` builders). Unauthenticated
   because the sandbox's `curl` can't carry the OAuth bearer token — but the
   `upload_id` itself is the capability: unguessable, single-use, expires in
   5 minutes, and can only ever have been minted by a call to
   `request_audio_upload` that already passed OAuth. Streams the body to a
   temp file with the same byte-cap-while-streaming behavior already used in
   `download_to_temp_file`, rejecting early (413) rather than buffering
   unbounded bytes.

3. **Extend `transcribe_audio_file`'s input schema** to a `oneOf` /
   discriminated union: either the existing
   `{ download_url, file_id, ... }` (ChatGPT path, unchanged, existing tests
   keep passing) or a new `{ upload_id }` (Claude.ai path) that reads the
   already-landed temp file instead of doing an HTTP GET. This keeps the
   tool count at +1 total (`request_audio_upload`), consistent with
   AGENTS.MD's "keep number of tools as few as possible."

No changes needed in `brain3-mcp-vault-tools` (Python container) —
transcription is a native in-gateway Rust tool, unrelated to that codebase.

## Required threat-model update (must happen before/with implementation)

`SECURITY_AUDIT.md`'s Threat Model section needs new entries before this
ships, per AGENTS.MD:

- **New asset**: in-flight uploaded audio bytes sitting in the upload
  registry/temp dir between `request_audio_upload` and `transcribe_audio_file`.
- **New trust boundary**: `PUT /uploads/{upload_id}` is reachable by anyone
  who can reach the public gateway origin, without an OAuth bearer token —
  the security boundary shifts from "bearer token" to "possession of a
  live, unguessable, single-use, 5-minute-TTL token." Needs explicit
  reasoning for why that's an acceptable trade-off (same style as the
  existing `download_url` SSRF-adjacent acceptance already documented for
  Finding-adjacent assumptions).
- **New attacker capability**: an attacker who can observe/guess an
  `upload_id` within its 5-minute window could PUT arbitrary bytes that get
  whisper-decoded — mitigated by the existing size cap + audio decode
  validation, same as today's `download_url` path.
- Note the token is generated with a CSPRNG (e.g. reuse whatever the
  existing OAuth `RandomGenerator`/token generation already uses in this
  codebase, don't hand-roll).

## Operational dependency worth flagging to the user (not a code change)

The Cloudflare **quick tunnel** (SECURITY_AUDIT.md Finding [3]) gets a new
URL on every gateway restart. Claude.ai's "Additional allowed domains"
allowlist is keyed by hostname, so a quick tunnel would force re-whitelisting
the domain after every restart for the sandbox `curl` to succeed. This flow
works far better with a **named/persistent** Cloudflare tunnel domain. Worth
calling out to the user as a prerequisite, not something to silently work
around in code.

## Implementation steps

1. Update `SECURITY_AUDIT.md` Threat Model (assets / trust boundaries /
   attacker capabilities) per above.
2. Add an in-memory upload registry (`Mutex<HashMap<upload_id, Entry>>` or
   similar), `Entry { expires_at, used: bool, path: Option<PathBuf> }`,
   likely as a small adapter under `crates/platform/src/` with lazy
   expiry-sweep on access (mirrors the Python script's `_uploads` dict but
   safe and bounded — needs eventual cleanup of orphaned temp files for
   uploads that were reserved but never PUT to, and never transcribed).
3. Add `request_audio_upload` `NativeMcpTool` impl, registered alongside
   `WhisperTranscribeTool` in `native_mcp_tools_from_config` (`apps/gateway/src/server.rs`).
4. Add `PUT /uploads/{upload_id}` handler in `crates/platform/src/http/`,
   wired into `router.rs`'s `build_router` (must be reachable through the
   public tunnel-facing router, same as `/oauth/*`).
5. Extend `WhisperToolArguments`/`OpenAIFileReferenceInput` deserialization
   in `whisper_transcribe.rs` to accept the `upload_id` variant, sharing the
   existing decode/size-limit/inference path; update `input_schema()` and
   the OpenAI `_meta` handling so it's additive, not replacing, the existing
   ChatGPT shape.
6. Tests (per AGENTS.MD: core behavior only, no tests on tool description
   strings): upload happy path, size-limit rejection while streaming,
   unknown/expired upload_id, single-use enforcement (second PUT and second
   transcribe both rejected after first success), `transcribe_audio_file`
   accepting `upload_id` end-to-end with a fake transcriber (same pattern as
   existing tests in `whisper_transcribe.rs`).
7. Local verification per AGENTS.MD: `cargo test -p brain3 --no-run`, then
   `cargo test`. Given this touches the gateway's public HTTP surface, also
   run the e2e smoke suite (`uv run scripts/e2e_smoke.py`) since AGENTS.MD
   calls that out explicitly for gateway/proxy changes.

## Open questions for you before I implement

- Confirm you're on an individual Claude.ai plan (Free/Pro/Max) — the Team-plan
  allowlist bug (`claude-code#63182`) would make this unreliable on Team.
- Confirm you're willing to move to a named Cloudflare tunnel if not already
  (needed so "Additional allowed domains" survives restarts).
- OK with the upload endpoint being unauthenticated-but-capability-scoped
  (token-gated) rather than bearer-token gated? That's the only way to make
  the sandbox's `curl` work at all, but it is a new ingress shape worth your
  explicit sign-off given the security posture of this repo.
