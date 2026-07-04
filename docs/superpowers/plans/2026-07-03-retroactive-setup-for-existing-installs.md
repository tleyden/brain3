Plan: Retroactive re-setup for existing installations (audio transcription gap, generalized)

## Problem

The whisper transcription plan (`2026-07-03-native-mcp-plugin-whisper-transcription.md`) shipped a
new setup-wizard screen (`SetupStep::AudioTranscription`) and a new Summary field, but **existing
installations can never reach it**. Concretely, once `~/.brain3/.env` exists, every normal launch
(`brain3`, `brain3 --tui`) takes the `GatewayTuiLaunch::Configured` path
(`apps/gateway/src/tui/app.rs:78-106`), which calls `start_without_writing_env` and jumps straight
to `SetupStep::RuntimeStatus` — it auto-starts the gateway and never enters the interactive wizard
screens at all. `FirstRunTuiState::new_configured` does set `state.step = SetupStep::Summary`
(`apps/gateway/src/tui/state.rs:201-215`), but that step is immediately overwritten before the
event loop ever draws a frame. There is currently **no key, flag, or menu path** that gets an
existing installation back into the wizard — not even the Cloudflare-only `--cf-setup` flag, which
is gated to named-tunnel provisioning only (`setup_requires_named_tunnel`,
`apps/gateway/src/main.rs:205-211`).

This means the audio transcription screen the previous plan built is currently **dead code** for
every user who set up Brain3 before this feature existed. That's the bug the user flagged, and it
will recur for any future setting added to the wizard — so the fix should be a reusable mechanism,
not a one-off for whisper.

### A second, independent bug this surfaced

While tracing this, I found the config loader's default for the new flag is actively wrong and
contradicts the docs:

- `crates/platform/src/config/env_file.rs:262-265`: when `B3_NATIVE_AUDIO_TRANSCRIPTION_ENABLED` is
  **absent** from `.env`, it defaults to `true` on macOS/Linux (`cfg!(any(target_os = "macos",
  target_os = "linux"))`).
- `README.md:464` documents the default as `false`.
- The first-run wizard's own default is `false` (`native_audio_transcription_default()` in
  `crates/core/src/application/first_run_setup.rs:230-232`) — finalize always writes the field
  explicitly, so `true` is reachable in practice only via this legacy-absent-key fallback.

Net effect: every pre-existing `.env` (written before this feature shipped) is missing the key, so
on upgrade the gateway silently flips native audio transcription **on** and registers
`transcribe_audio_file` in `tools/list` (`apps/gateway/src/server.rs:424-444` has no check that the
model file actually exists at `transcription.model_path`). Since the model was never downloaded
(the wizard step that downloads it never ran for these installs), the tool is advertised to AI
clients but will fail the first time anything calls it. This is a real, shipping bug independent of
the wizard-navigation gap, and needs fixing regardless of what we do below.

---

## Fixes

### 1. Fix the unsafe default (bug, do first)

`crates/platform/src/config/env_file.rs:262-265` — change the default passed to `env_bool` from
`cfg!(any(target_os = "macos", target_os = "linux"))` to `false`, matching both the documented
default in README.md and the wizard's own default. "Absent key" should mean "written before this
setting existed," not "silently opt this install into a multi-hundred-MB feature it never
consented to and never downloaded a model for."

Add a regression test in `env_file.rs`'s existing test module: with the env var unset,
`load_native_audio_transcription_config` returns `enabled: false` regardless of target OS.

### 2. Add a defense-in-depth guard against advertising a broken tool

`apps/gateway/src/server.rs:424-444` (`native_mcp_tools_from_config`) — even with fix #1, a user
could hand-edit `.env` to `B3_NATIVE_AUDIO_TRANSCRIPTION_ENABLED=true` without the model file
present (partial download, manual edit, moved `~/.brain3` directory, etc.). Add a check: if
`enabled` but `!transcription.model_path.exists()`, log `tracing::warn!` with the expected path and
a pointer to re-run setup, and return an empty tool list (functionally disabled) instead of
registering a tool that will error on first real call. Add a unit test for this branch (function
already returns `Result<Vec<Arc<dyn NativeMcpTool>>>`, so it's directly testable without spinning
up a server).

### 3. New `brain3 --setup` flag: the actual re-entry mechanism

This is the missing piece. Good news: essentially all the UI already exists and works — the
`Summary` screen already renders "Native audio transcription" and "Whisper model" as inline,
toggleable, tab-focusable fields (`apps/gateway/src/tui/screens.rs:800-814`), `Esc` from `Summary`
already walks backward through every screen including `AudioTranscription`
(`FirstRunTuiState::previous_step`, `apps/gateway/src/tui/state.rs:651-667`), and `Enter` on
`Summary` already calls `finalize_and_start`, which writes `.env` and downloads the selected
whisper model if newly enabled (`FirstRunSetupUseCase::finalize`,
`crates/core/src/application/first_run_setup.rs:141-147`). **The only missing piece is an entry
point that reaches `SetupStep::Summary` interactively instead of auto-starting past it.**

Changes, all in `apps/gateway/src/main.rs` and `apps/gateway/src/tui/app.rs`:

- **`Args`** (`main.rs:64-101`): add `#[arg(long, conflicts_with_all = ["tui", "cli", "cf_setup"])] setup: bool`.
- **`LaunchMode`** (`main.rs:138-143`): add a `Reconfigure` variant. `choose_launch_mode` returns it
  when `args.setup` is set.
- **`plan_launch`** (`main.rs:228-...`): for `LaunchMode::Reconfigure`, mirror the existing
  `LaunchMode::Setup` (`--cf-setup`) handling shape but for the general case: if `!env_exists`,
  error out telling the user there's nothing to reconfigure yet — just run `brain3` for first-run
  setup (parallel to `setup_requires_existing_config`, `main.rs:532-549`). If `env_exists`, produce
  a new `LaunchDispatch::TuiReconfigure { launch_plan }`.
- **`main()`** (`main.rs:675-...`): add a `LaunchDispatch::TuiReconfigure` arm. Reuse the same
  cloudflared-named-tunnel preamble the `TuiConfigured` arm already has (so `--setup` on a named
  Cloudflare tunnel install still detects a missing tunnel config file correctly), then call
  `tui::run_gateway_tui(..., GatewayTuiLaunch::Reconfigure { launch_plan }, ...)` instead of
  `Configured`. Requires an interactive terminal, same as `TuiConfigured` today.
- **`GatewayTuiLaunch`** (`tui/app.rs:36`): add a `Reconfigure { launch_plan: RuntimeLaunchPlan }`
  variant.
- **`run_gateway_tui`** (`tui/app.rs:65-107`): add a match arm for `Reconfigure` that does exactly
  what the `Configured` arm does (load config, `prepare_from_existing_config`, `new_configured`)
  **except it must not call `start_without_writing_env`**. Instead return
  `(state, RuntimeStartupPolicy::setup_or_reconfigure(), None)` — the same policy tuple shape
  `FirstRun` uses, since "about to review and possibly rewrite config, then start" is the same
  situation as first-run, not a plain configured launch. `state.step` is already `Summary` from
  `new_configured`; the existing event loop takes over from there with zero further changes needed.

That's the entire mechanism. No new TUI screens, no new state fields, no new finalize logic —
just a new entry point into code that's already built and tested.

#### Naming check (flagging for your call, not deciding for you)

`--setup` sits right next to the existing `--cf-setup` (Cloudflare named-tunnel-only wizard) and
could read as a broader superset of it, which might confuse users about which one to reach for.
`--reconfigure` is less ambiguous but longer. I'd lean `--setup` since you proposed it and it's the
more discoverable/guessable name, but wanted to flag the adjacency before locking it in.

### 4. Discoverability of the new flag itself

Adding `--setup` doesn't help if nobody knows it exists. Two lightweight, non-generic additions
(deliberately not building a config-schema-versioning framework — see Non-goals):

- **README**: document `brain3 --setup` under Advanced Configuration / Quick Start as "change any
  existing setting, including enabling audio transcription on an install that predates it." Update
  `--help` text (`main.rs` clap `help = "..."` attribute on the new arg) accordingly.
- **One-time startup nudge**: in the `TuiConfigured` auto-start path (`main.rs`, where `.env` is
  read), check whether the raw `.env` file text contains the literal key
  `B3_NATIVE_AUDIO_TRANSCRIPTION_ENABLED=` (a plain substring check on the file contents already
  read for `dotenvy`, not a schema-version system). If absent, log a single
  `tracing::info!("native audio transcription is available but not yet configured on this \
  install; run `brain3 --setup` to enable it")`. This surfaces once, in the same logs the
  RuntimeStatus screen already shows, without inventing new infrastructure. Future settings can
  repeat this same one-line pattern rather than needing a generic mechanism.

---

## Non-goals

- **No generic config-schema-versioning framework.** The user anticipated needing this repeatedly
  ("we probably need a re-setup for many changes"), and `--setup` is exactly that reusable
  mechanism — it's general-purpose by construction (it re-enters the *entire* wizard against
  existing config, not just the audio transcription screen), so no per-field migration machinery is
  needed. The one-time log nudge in #4 is a plain substring check, not a versioned framework; add
  another one-line check like it if/when the next setting needs the same nudge.
- **No forced/blocking re-setup.** `--setup` is opt-in. We are not making existing installs fail to
  start or forcing a wizard interruption on normal launch — that would be a bigger behavior change
  than this gap warrants, and conflicts with "keep main.rs lean."
- **No threat-model changes.** This doesn't add a new trust boundary or egress path beyond what the
  original whisper plan already covered in `SECURITY_AUDIT.MD` — `--setup` reuses the exact same
  `finalize()` path (including its existing model download + checksum verification) that first-run
  setup already goes through today.

---

## Phased implementation

1. `crates/platform/src/config/env_file.rs`: fix the unsafe default (item 1) + test.
2. `apps/gateway/src/server.rs`: add the model-file-exists guard (item 2) + test.
3. `apps/gateway/src/main.rs`: add `--setup` flag, `LaunchMode::Reconfigure`, `plan_launch` arm,
   `LaunchDispatch::TuiReconfigure` arm, error guard for missing `.env` + tests (mirroring existing
   `choose_launch_mode`/`plan_launch` test patterns already in `main.rs`).
4. `apps/gateway/src/tui/app.rs`: add `GatewayTuiLaunch::Reconfigure` variant + match arm in
   `run_gateway_tui`.
5. `main.rs`: add the one-time startup log nudge (item 4).
6. `README.md`: document `--setup`, update `--help` text.
7. `cargo test -p brain3 --no-run` + `cargo test`.
8. Manual verification: hand-write a legacy-style `.env` missing all three `B3_NATIVE_AUDIO_*`
   /`B3_WHISPER_*` keys, confirm (a) normal `brain3 --tui` launch auto-starts as before and logs the
   nudge, (b) `tools/list` does not include `transcribe_audio_file` (fix #1 + #2 both hold), (c)
   `brain3 --setup` opens directly to the `Summary` screen with existing values populated, (d)
   toggling "Native audio transcription" on and hitting Enter downloads the model and starts the
   gateway with the tool now present in `tools/list`, (e) `brain3 --setup` with no `.env` present
   errors with guidance to run plain `brain3` instead.


Also rename --cf-setup`  to --cloudflare-setup in any places we missed