# Plan: Rename to "Network Security" + Add Container Name/Network Name Fields

## Goal

1. Rename the "Ports & Settings" step to "Network Security" throughout the TUI.
2. Add two new editable fields: `container name` and `container network name`.
3. Hide `container network name` when `Internal-only container networking` is Disabled.

---

## 1. `crates/core/src/domain/setup.rs`

- Add constants: `DEFAULT_CONTAINER_NAME = "brain3-mcp-vault-tools"` and `DEFAULT_CONTAINER_NETWORK_NAME = "brain3-mcp-net"`
- Add `container_name: String` and `container_network_name: String` to `SetupDraftConfig`

---

## 2. `crates/core/src/application/first_run_setup.rs`

- Initialize the two new draft fields with the new constants
- Update `SetupDraftConfig` literals in tests to include the new fields

---

## 3. `apps/gateway/src/tui/state.rs`

- Add `ContainerName` and `ContainerNetworkName` to `PortsField` and `SummaryField` enums
- Add `container_name_input: String` and `container_network_name_input: String` to `FirstRunTuiState`
- Initialize those inputs from `preparation.draft.container_name/container_network_name` in `new()`
- Sync inputs back to draft in `apply_inputs_to_draft()`
- Add the new variants to `ports_focus_is_text_field()` and `summary_focus_is_text_field()`
- Update `ports_focus_order()`: add `container_network_isolated: bool` parameter; insert `ContainerName` after `ContainerMcpPort` in all modes; append `ContainerNetworkName` after `ContainerNetworkIsolation` only when `container_network_isolated == true`
- Update the three callers of `ports_focus_order()` to pass the new parameter
- Update `summary_focus_order()` and its callers similarly
- Add `ContainerName` and `ContainerNetworkName` arms to `summary_char_push()` and `summary_char_pop()`
- Update tests: add the two new fields to `sample_state()`'s `SetupDraftConfig`; update the expected `ports_focus` and `summary_focus` sequences in assertions to include the new fields

---

## 4. `apps/gateway/src/tui/app.rs`

- Add backspace/char push cases for `PortsField::ContainerName` and `PortsField::ContainerNetworkName` in the `PortsAndSettings` key-handler

---

## 5. `apps/gateway/src/tui/screens.rs`

- Rename step title: `"Ports & Settings"` → `"Network Security"`
- Update `progress_caption` for `PortsAndSettings` to match
- Update the three `muted_line` headers in `ports_and_settings_lines()` to reflect the new "Network Security" framing
- Add a `container_name` field line after `ContainerMcpPort`
- Add a `container_network_name` field line after the `ContainerNetworkIsolation` block, hidden when `state.draft.container_network_isolated == false`
- Update `summary_lines()` to display `container_name` and (conditionally) `container_network_name`
- Update tests: add assertions for the new field labels; update assertion counts/sequences

---

## 6. `crates/platform/src/setup/env_writer.rs`

- In `build_overrides()`: insert `B3_CONTAINER_NAME` and `B3_CONTAINER_NETWORK_NAME` from `draft.container_name` / `draft.container_network_name`

---

## 7. Other `SetupDraftConfig` literal sites

- `crates/platform/tests/setup_bootstrap.rs` — add the two new fields to any `SetupDraftConfig { ... }` literals
- `crates/platform/src/setup/system.rs` and `apps/gateway/src/tui/screens.rs` test helpers — same
