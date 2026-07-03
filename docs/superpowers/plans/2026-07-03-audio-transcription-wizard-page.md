# Audio Transcription wizard page, off-by-default, more Whisper models

## Goals

1. `native_audio_transcription_enabled` defaults to **disabled**.
2. `Whisper max audio bytes` is removed from the onboarding UI entirely (both
   the wizard step and the Summary screen) but stays a supported `.env`
   variable (`B3_WHISPER_MAX_AUDIO_BYTES`) that power users can hand-edit.
3. Add more popular Whisper model choices beyond `tiny.en` / `base.en`, with
   size shown in parens next to each model.
4. Move Whisper model selection (and the native-transcription toggle) out of
   "Network Security" into its own wizard step, **"Audio Transcription"**,
   placed right after "Network Security" and before "Summary". The new page
   gets descriptive copy explaining what the feature is and pointing to the
   README for details.

## 1. Disable native audio transcription by default

`crates/core/src/application/first_run_setup.rs:230`

```rust
fn native_audio_transcription_default() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}
```

Change to simply return `false`. Update the existing test(s) around
`first_run_setup.rs:560` and any other assertion that assumes the default is
`true` (search for `native_audio_transcription_enabled` in tests).

## 2. Remove "Whisper max audio bytes" from onboarding UI

Keep `whisper_max_audio_bytes` in `SetupDraftConfig`, `DEFAULT_WHISPER_MAX_AUDIO_BYTES`,
`env_writer.rs` (still written to `.env` using the default), and
`env_file.rs` (`B3_WHISPER_MAX_AUDIO_BYTES` still read at runtime) — none of
that changes. Only remove it from the TUI:

- `apps/gateway/src/tui/screens.rs`: delete the `field_line("Whisper max audio bytes", ...)`
  block from `ports_and_settings_lines` (screens.rs:630-634) and from
  `summary_lines` (screens.rs:801-805), plus the trailing muted hint line
  that references it if it no longer applies.
- `apps/gateway/src/tui/state.rs`: remove `PortsField::WhisperMaxAudioBytes`
  and `SummaryField::WhisperMaxAudioBytes` variants, their entries in
  `ports_focus_order` / `summary_focus_order`, `ports_focus_is_text_field`,
  `ports_focus_is_digits_only`, `summary_focus_is_text_field`,
  `summary_focus_is_digits_only`, `summary_char_push`, `summary_char_pop`.
  Remove `whisper_max_audio_bytes_input` field and its construction in `new()`.
  Remove the parse-and-assign block in `apply_inputs_to_draft`
  (state.rs:251-253) — the draft keeps whatever default/loaded value it had.
- `apps/gateway/src/tui/app.rs`: remove the `WhisperMaxAudioBytes` validation
  branch in the `PortsAndSettings` Enter handler (app.rs:329-334 and
  348-353), and the `Backspace`/`Char` match arms for
  `PortsField::WhisperMaxAudioBytes` (app.rs:398-400, 423-425).
- Update affected tests in `state.rs` (`both_mode_ports_focus_...`,
  `*_summary_focus_*` tests currently assert `WhisperMaxAudioBytes` is last
  in the order — drop it from expected vectors) and any `screens.rs` /
  `app.rs` tests that reference the field or type into it.

## 3. Expand Whisper model choices, with size shown

Today the model list lives in two places that must stay in sync:

- `crates/platform/src/setup/system.rs:31` — `WHISPER_MODEL_SPECS` (filename,
  download URL, sha256, size in bytes) used to actually download/verify the
  model.
- `apps/gateway/src/tui/state.rs:873` — `toggle_whisper_model` which just
  flips between the two hardcoded strings.

Plan:

- Add `small.en` and `medium.en` to `WHISPER_MODEL_SPECS` (staying
  English-only, consistent with the two models already offered and with the
  tool's English-only framing). Real `sha256` + `size_bytes` need to be
  pulled from the `ggerganov/whisper.cpp` Hugging Face repo at
  implementation time — I'll fetch these before writing the code so the
  checksums are correct (placeholder values would break first-run setup).
  Sizes to display: `tiny.en` ~75 MB, `base.en` ~142 MB, `small.en` ~466 MB,
  `medium.en` ~1.5 GB. (Not proposing `large-v3`/`large-v3-turbo` — multi-GB
  downloads and multilingual vocab are a bigger jump; can add later if
  wanted.)
- Replace the boolean `toggle_whisper_model` with a proper cycle-through-list
  helper, e.g.:
  ```rust
  const WHISPER_MODEL_CHOICES: &[(&str, &str)] = &[
      ("tiny.en", "75 MB"),
      ("base.en", "142 MB"),
      ("small.en", "466 MB"),
      ("medium.en", "1.5 GB"),
  ];

  fn cycle_whisper_model(model: &mut String, forward: bool) { ... }
  ```
  Used by both the new Audio Transcription page and the Summary screen (same
  as today's `toggle_whisper_model` call sites in `state.rs:372` and `:580`).
- Render as `badge_span("base.en (142 MB)", Color::Cyan)` (or a small helper
  `whisper_model_label(model) -> String` shared by `screens.rs`) instead of
  the bare model string, in both the new wizard page and the Summary line.
- Keep the `'t'` key toggling forward through the list (matches the existing
  single-key convention for this field); no need for a second "previous"
  binding given the existing UI pattern for other cycled fields.

## 4. New "Audio Transcription" wizard step

### Domain / step plumbing

- `crates/core/src/domain/setup.rs:125` — add `SetupStep::AudioTranscription`
  right after `PortsAndSettings` in the enum.
- `apps/gateway/src/tui/screens.rs`:
  - `progress_lines` stages array (`screens.rs:69-78`): insert
    `"Audio Transcription"` after `"Network Security"`.
  - `screen_title`, `progress_caption`, `wizard_stage_index`, `body_lines`:
    add matches for the new step (title: `"Audio Transcription"`; caption
    something like `"Enable native audio transcription and choose a Whisper model."`).
    Bump `wizard_stage_index` values for `Summary`/`ConnectionCard`/etc. by 1.
  - Add `audio_transcription_lines(state)` rendering:
    - Descriptive copy (muted lines) explaining the feature, e.g.:
      > "Audio Transcription is an MCP tool that runs natively in this
      > process. Drag an audio file into your AI assistant and it gets
      > transcribed automatically — no external service required."
      > "See README.md for more details."
    - The `Native audio transcription` enabled/disabled badge field (moved
      from `ports_and_settings_lines`).
    - The `Whisper model` badge field with size-annotated label (moved from
      `ports_and_settings_lines`), only meaningfully interactive when
      transcription is enabled (keep it visible either way, consistent with
      how other conditional fields behave elsewhere in this file).
    - Drop the `"The selected model is downloaded and checksum-verified..."`
      hint line down to this page.
  - `ports_and_settings_lines`: remove the "Native audio transcription" /
    "Whisper model" / "Whisper max audio bytes" block (screens.rs:615-637)
    entirely — Network Security goes back to being just ports/container/token
    settings.
  - `action_lines`: add a case for `AudioTranscription` mirroring
    `PortsAndSettings`'s continue-style hints (`[Tab]`/`[t] Toggle`/`[Esc]`/`[q]`/`[Enter]`).

### State plumbing (`apps/gateway/src/tui/state.rs`)

- New `PortsField`-style focus enum for the page, e.g. `AudioTranscriptionField { NativeAudioTranscription, WhisperModel }`
  (or reuse a trimmed `PortsField` — cleaner to introduce a dedicated enum
  since `PortsField` no longer needs these variants after step 4 removes them
  from Network Security).
- `PortsField::NativeAudioTranscription` / `PortsField::WhisperModel` move
  out of `PortsField` into this new enum; remove their entries from
  `ports_focus_order` (state.rs:816-818) so Network Security's tab order no
  longer includes them.
- Add `audio_transcription_focus: AudioTranscriptionField` to
  `FirstRunTuiState`, plus `next_audio_transcription_focus` /
  `previous_audio_transcription_focus` (two-item cycle, same shape as
  `next_access_mode_focus`).
- `toggle_ports_boolean`-equivalent for the new page (toggle native
  transcription, cycle whisper model on `'t'`).
- `SummaryField::NativeAudioTranscription` / `SummaryField::WhisperModel`
  stay as-is (Summary continues to show/edit both, same as today) — only
  `WhisperMaxAudioBytes` is removed per step 2.

### Navigation (`apps/gateway/src/tui/app.rs`)

- `PortsAndSettings` Enter handler: instead of going straight to `Summary`,
  go to `SetupStep::AudioTranscription` (after existing port/token
  validation, minus the whisper-max-bytes validation branch being deleted).
- New `SetupStep::AudioTranscription` match arm:
  - `Esc` → back to `PortsAndSettings`.
  - `Tab`/`Up`/`Down` → cycle `audio_transcription_focus`.
  - `Char('t')` → toggle/cycle current field.
  - `Enter` → `SetupStep::Summary`.
- `Summary` step's `Esc` currently goes to `PortsAndSettings` (app.rs:434) —
  change to `SetupStep::AudioTranscription` so back-navigation follows the
  new order.
- `previous_step()` in `state.rs:637-656`: add
  `SetupStep::AudioTranscription => Some(SetupStep::PortsAndSettings)`, and
  change `SetupStep::Summary => Some(SetupStep::PortsAndSettings)` to
  `Some(SetupStep::AudioTranscription)`.

## 5. Docs

- `README.md:135` — update the "Network Security" bullet to drop the
  audio-transcription mention, and add a new "Audio Transcription" bullet
  describing the step.
- `README.md:464` — update the `B3_WHISPER_MODEL` table row to list all
  four supported values (`tiny.en`, `base.en`, `small.en`, `medium.en`).
- `README.md:465` — note that `B3_WHISPER_MAX_AUDIO_BYTES` is `.env`-only
  (no longer set via the wizard).

## 6. Tests to update

- `crates/core/src/application/first_run_setup.rs`: default-value test
  (native transcription now defaults to `false`); anywhere sample drafts set
  `native_audio_transcription_enabled: true` intentionally for
  download-related tests should keep doing so explicitly.
- `apps/gateway/src/tui/state.rs` unit tests: `ports_focus_order` /
  `summary_focus_order` expectation vectors (remove `WhisperMaxAudioBytes`,
  remove `NativeAudioTranscription`/`WhisperModel` from the `PortsField`
  sequence since they move to the new focus enum), add tests for the new
  `AudioTranscriptionField` cycle.
- `apps/gateway/src/tui/screens.rs` unit tests (screens.rs:1684-2010-ish):
  update header/progress/stage-count assertions for the new 9-stage wizard
  (was `Step X of 8`, becomes `Step X of 9`), and any fixture reading
  `whisper_max_audio_bytes` from the rendered ports screen.
- `apps/gateway/src/tui/app.rs` tests referencing `SetupStep::PortsAndSettings`
  → `SetupStep::Summary` transitions need to route through the new
  `AudioTranscription` step instead.
- `apps/gateway/tests/e2e_smoke.rs` — check for any step-count or
  key-sequence assumptions tied to the wizard flow.

## 7. Verification

- `cargo test -p brain3 --no-run` then `cargo test` (per project convention).
- Manually run the setup wizard (`cargo run` or equivalent dev entrypoint) to
  click through: Network Security → Audio Transcription (verify default is
  Disabled, cycle through all 4 models with sizes shown, toggle enabled) →
  Summary (confirm no "Whisper max audio bytes" row, confirm model/enabled
  reflect the new page) → Esc back-navigation returns to Audio Transcription,
  not Network Security.
- E2E smoke test only if the change ends up touching gateway/runtime
  bootstrap beyond the TUI (it shouldn't, but re-check `bootstrap.rs:183-184`
  and `app.rs:1010/1108/1215` fixtures still compile since they hardcode
  `"base.en"`).

## Open question for you

Should the new model list stay English-only (`tiny.en`/`base.en`/`small.en`/
`medium.en`) or do you also want multilingual variants (`tiny`/`base`/etc.)?
Defaulting to English-only in this plan since that's what's there today and
it keeps the tool/download list simple, but flagging it since "popular
options" could reasonably include multilingual too.
