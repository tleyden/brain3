use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use brain3_core::domain::model::ContainerRuntime;
use brain3_core::domain::setup::{
    DependencyAvailability, PackageManager, SetupStep, TunnelModeDraft,
};
use brain3_platform::runtime::StartupStatus;

use crate::release;
use crate::server::GatewayServerStatus;

use super::runtime_logs::RuntimeLogsState;
use super::state::{
    install_action_label, AuthField, DependencyDoctorFocus, FirstRunTuiState, PortsField,
    RuntimeView, SummaryField,
};

pub fn draw(f: &mut ratatui::Frame, state: &FirstRunTuiState) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(6),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(5),
        ])
        .split(area);

    let header = Paragraph::new(header_line(state))
        .block(panel_block("brain3"))
        .wrap(Wrap { trim: false });
    f.render_widget(header, chunks[0]);

    let progress = Paragraph::new(progress_lines(state))
        .block(panel_block("Wizard"))
        .wrap(Wrap { trim: false });
    f.render_widget(progress, chunks[1]);

    render_body(f, state, chunks[2]);

    let status = Paragraph::new(status_lines(state))
        .block(panel_block("Status"))
        .wrap(Wrap { trim: false });
    f.render_widget(status, chunks[3]);

    let footer = Paragraph::new(action_lines(state))
        .block(panel_block("Actions"))
        .wrap(Wrap { trim: false });
    f.render_widget(footer, chunks[4]);
}

fn header_line(state: &FirstRunTuiState) -> Line<'static> {
    Line::from(vec![
        Span::styled(release::APP_VERSION_DISPLAY, heading_style()),
        Span::styled("  •  ", muted_style()),
        Span::styled(screen_title(state.step).to_string(), value_style()),
    ])
}

fn progress_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let current_index = wizard_stage_index(state.step);
    let stages = [
        "Welcome",
        "Dependencies",
        "Vault",
        "Auth",
        "Ports",
        "Start",
        "Running",
    ];

    let mut stage_spans = Vec::new();
    for (index, stage) in stages.iter().enumerate() {
        if index > 0 {
            stage_spans.push(Span::styled(" ─ ", muted_style()));
        }

        let stage_span = if index < current_index {
            Span::styled(format!("✓ {stage}"), success_style())
        } else if index == current_index {
            Span::styled(
                format!("● {stage}"),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!("○ {stage}"), muted_style())
        };
        stage_spans.push(stage_span);
    }

    vec![
        Line::from(vec![
            Span::styled(
                format!("Step {} of {}", current_index + 1, stages.len()),
                accent_style(),
            ),
            Span::styled("  ", muted_style()),
            Span::styled(progress_caption(state.step).to_string(), muted_style()),
        ]),
        Line::from(stage_spans),
        blank_line(),
        muted_line("Navigation Controls At Bottom Of Screen"),
    ]
}

fn screen_title(step: SetupStep) -> &'static str {
    match step {
        SetupStep::Welcome => "Welcome",
        SetupStep::DependencyDoctor => "Dependency Doctor",
        SetupStep::VaultPath => "Vault Path",
        SetupStep::Auth => "Auth Setup",
        SetupStep::PortsAndSettings => "Ports & Settings",
        SetupStep::Summary => "Summary",
        SetupStep::ConnectionCard => "MCP Config Settings",
        SetupStep::RuntimeStatus => "Runtime Status",
    }
}

fn body_panel_title(state: &FirstRunTuiState) -> &'static str {
    match state.step {
        SetupStep::ConnectionCard => "MCP Config",
        SetupStep::RuntimeStatus if state.runtime_view == RuntimeView::Logs => "Logs",
        SetupStep::RuntimeStatus => "Runtime Status",
        _ => "Details",
    }
}

fn progress_caption(step: SetupStep) -> &'static str {
    match step {
        SetupStep::Welcome => "Start with the defaults Brain3 prepared for this machine.",
        SetupStep::DependencyDoctor => "Confirm local dependencies and install anything missing.",
        SetupStep::VaultPath => "Choose the Obsidian-compatible vault Brain3 should expose.",
        SetupStep::Auth => "Review the generated login defaults and customize them if needed.",
        SetupStep::PortsAndSettings => {
            "Override ports and security settings if the defaults conflict."
        }
        SetupStep::Summary => "Confirm what Brain3 will write before startup begins.",
        SetupStep::ConnectionCard | SetupStep::RuntimeStatus => {
            "Brain3 is configured. Use the connection details or monitor runtime status."
        }
    }
}

fn wizard_stage_index(step: SetupStep) -> usize {
    match step {
        SetupStep::Welcome => 0,
        SetupStep::DependencyDoctor => 1,
        SetupStep::VaultPath => 2,
        SetupStep::Auth => 3,
        SetupStep::PortsAndSettings => 4,
        SetupStep::Summary => 5,
        SetupStep::ConnectionCard | SetupStep::RuntimeStatus => 6,
    }
}

fn body_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    match state.step {
        SetupStep::Welcome => welcome_lines(state),
        SetupStep::DependencyDoctor => dependency_lines(state),
        SetupStep::VaultPath => vault_lines(state),
        SetupStep::Auth => auth_lines(state),
        SetupStep::PortsAndSettings => ports_and_settings_lines(state),
        SetupStep::Summary => summary_lines(state),
        SetupStep::ConnectionCard => connection_card_lines(state),
        SetupStep::RuntimeStatus => runtime_lines(state),
    }
}

fn render_body(f: &mut ratatui::Frame, state: &FirstRunTuiState, area: Rect) {
    if state.step == SetupStep::RuntimeStatus && state.runtime_view == RuntimeView::Logs {
        render_runtime_logs(f, state, area);
        return;
    }

    let body = Paragraph::new(body_lines(state))
        .block(panel_block(body_panel_title(state)))
        .wrap(Wrap { trim: false });
    f.render_widget(body, area);
}

fn render_runtime_logs(f: &mut ratatui::Frame, state: &FirstRunTuiState, area: Rect) {
    let block = panel_block(body_panel_title(state));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    let mode_line = if state.runtime_logs.is_following() {
        Line::from(vec![
            badge_span("LIVE", Color::Green),
            Span::styled("  Following newest retained log lines.", muted_style()),
        ])
    } else {
        Line::from(vec![
            badge_span("SCROLLED", Color::Yellow),
            Span::styled("  Press End to jump back to the live tail.", muted_style()),
        ])
    };
    f.render_widget(Paragraph::new(mode_line), sections[0]);

    match state.runtime_logs.state() {
        RuntimeLogsState::Unavailable(message) => {
            let body = Paragraph::new(vec![
                muted_line("Runtime logs are temporarily unavailable."),
                blank_line(),
                Line::from(Span::styled(message.clone(), warning_style())),
            ]);
            f.render_widget(body, sections[1]);
        }
        RuntimeLogsState::Empty | RuntimeLogsState::Loading => {
            let message = match state.runtime_logs.state() {
                RuntimeLogsState::Loading => "Loading runtime logs...",
                _ => "No complete log lines have been written yet.",
            };
            f.render_widget(Paragraph::new(vec![muted_line(message)]), sections[1]);
        }
        RuntimeLogsState::Ready => {
            let log_lines: Vec<Line<'static>> = state
                .runtime_logs
                .lines()
                .iter()
                .cloned()
                .map(Line::from)
                .collect();
            let scroll = state
                .runtime_logs
                .scroll_offset_for_height(sections[1].height as usize);
            let body = Paragraph::new(log_lines).scroll((scroll, 0));
            f.render_widget(body, sections[1]);
        }
    }
}

fn welcome_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    vec![
        muted_line("This guided setup writes the default Brain3 config and starts the gateway."),
        muted_line("The same shell is also used later for everyday runtime status."),
        blank_line(),
        Line::from(Span::styled("Important locations:", accent_style())),
        blank_line(),
        key_value_line(
            "App home",
            state.preparation.paths.app_home.display().to_string(),
        ),
        key_value_line(
            "Env file",
            state.preparation.paths.env_file.display().to_string(),
        ),
        key_value_line("Logs", state.log_file.display().to_string()),
    ]
}

fn dependency_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let deps = &state.preparation.dependencies;
    let actions = state.dependency_actions();
    let selected_action_index = state.selected_dependency_action_index();
    let action_list_focused =
        matches!(state.dependency_focus, DependencyDoctorFocus::InstallAction);

    let mut lines = vec![
        muted_line("Brain3 checked the dependencies it can manage from this setup wizard."),
        blank_line(),
        key_value_line("Operating system", format!("{:?}", deps.operating_system)),
        key_value_line(
            "Package manager",
            format_package_manager(deps.package_manager),
        ),
        key_badge_line("cloudflared", dependency_badge(deps.cloudflared)),
        key_badge_line("Docker", bool_badge(deps.docker_installed)),
        key_badge_line(
            "macOS container",
            optional_bool_badge(deps.macos_container_installed),
        ),
        key_value_line(
            "Default runtime",
            format_container_runtime(state.draft.container_runtime),
        ),
    ];

    lines.push(blank_line());

    if actions.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("✓ ", success_style()),
            Span::styled(
                "All required dependencies are already available.",
                success_style(),
            ),
        ]));
        lines.push(muted_line(
            "You can continue, or refresh after making changes outside Brain3.",
        ));
        return lines;
    }

    lines.push(Line::from(Span::styled(
        "Available actions",
        section_heading_style(),
    )));

    for (index, action) in actions.iter().enumerate() {
        let is_selected = selected_action_index == Some(index);
        let marker = if is_selected && action_list_focused {
            Span::styled("▶ ", accent_style())
        } else if is_selected {
            Span::styled("• ", value_style())
        } else {
            Span::styled("  ", muted_style())
        };

        let label = install_action_label(*action).to_string();
        let label_style = if is_selected && action_list_focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            accent_style()
        } else {
            value_style()
        };

        lines.push(Line::from(vec![marker, Span::styled(label, label_style)]));
    }

    lines.push(blank_line());
    lines.push(muted_line(
        "Use Tab to switch between install actions and continue.",
    ));
    lines
}

fn vault_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    vec![
        muted_line("Enter the absolute path to your Obsidian-compatible vault."),
        muted_line("Brain3 will mount this path when it starts the local MCP container."),
        blank_line(),
        field_line("Vault path", &state.vault_path_input, true),
    ]
}

fn auth_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let password_line = if state.generate_password {
        "auto-generated".to_string()
    } else {
        "*".repeat(state.password_input.len())
    };

    let password_mode = if state.generate_password {
        badge_span("Auto-generated", Color::Green)
    } else {
        badge_span("Custom password", Color::Cyan)
    };

    let password_hint = if state.generate_password {
        "Press g to use a custom password."
    } else {
        "Press g to switch back to an auto-generated password."
    };

    vec![
        muted_line(
            "Client secret is generated automatically. Access and refresh tokens are issued per session.",
        ),
        muted_line("Username, client ID, and password settings stay local to this machine."),
        blank_line(),
        field_line(
            "Username",
            &state.username_input,
            state.auth_focus == AuthField::Username,
        ),
        field_line(
            "Client ID",
            &state.client_id_input,
            state.auth_focus == AuthField::ClientId,
        ),
        field_line(
            "Password",
            &password_line,
            state.auth_focus == AuthField::Password && !state.generate_password,
        ),
        blank_line(),
        Line::from(vec![label_span("Mode"), Span::raw(": "), password_mode]),
        muted_line(password_hint),
    ]
}

fn ports_and_settings_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let mut lines = vec![
        muted_line("Override ports if the defaults conflict with other services on this machine."),
        muted_line("Toggle security settings only if your client requires it."),
        muted_line("Disable internal-only networking only as a compatibility fallback."),
        blank_line(),
    ];

    lines.push(field_line(
        "Gateway port",
        &state.gateway_port_input,
        state.ports_focus == PortsField::GatewayPort,
    ));
    lines.push(field_line(
        "Container host port",
        &state.container_host_port_input,
        state.ports_focus == PortsField::ContainerHostPort,
    ));
    lines.push(field_line(
        "Container MCP port",
        &state.container_mcp_port_input,
        state.ports_focus == PortsField::ContainerMcpPort,
    ));
    lines.push(field_line(
        "Access token lifetime (secs)",
        &state.access_token_lifetime_secs_input,
        state.ports_focus == PortsField::AccessTokenLifetimeSecs,
    ));
    lines.push(field_line(
        "Refresh token lifetime (secs)",
        &state.refresh_token_lifetime_secs_input,
        state.ports_focus == PortsField::RefreshTokenLifetimeSecs,
    ));

    lines.push(blank_line());

    let pkce_active = state.ports_focus == PortsField::PkceRequired;
    let pkce_badge = if state.draft.pkce_required {
        badge_span("Enabled", Color::Green)
    } else {
        badge_span("Disabled", Color::Yellow)
    };
    let pkce_pointer = if pkce_active {
        Span::styled("▶ ", accent_style())
    } else {
        Span::styled("  ", muted_style())
    };
    lines.push(Line::from(vec![
        pkce_pointer,
        Span::styled(
            "PKCE required: ".to_string(),
            if pkce_active {
                accent_style()
            } else {
                label_style()
            },
        ),
        pkce_badge,
    ]));

    lines.push(field_badge_line(
        "Enforce hostname check",
        if state.draft.enforce_hostname_check {
            badge_span("Enabled", Color::Green)
        } else {
            badge_span("Disabled", Color::Yellow)
        },
        state.ports_focus == PortsField::EnforceHostnameCheck,
    ));
    lines.push(field_badge_line(
        "Internal-only container networking",
        if state.draft.container_network_isolated {
            badge_span("Enabled", Color::Green)
        } else {
            badge_span("Disabled", Color::Yellow)
        },
        state.ports_focus == PortsField::ContainerNetworkIsolation,
    ));
    lines.push(muted_line(
        "Enabled removes the MCP container's default outbound route for maximum isolation.",
    ));
    lines.push(muted_line(
        "Disabled uses the runtime's normal bridge/default network for VPS compatibility.",
    ));

    lines
}

fn summary_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let f = state.summary_focus;
    let mut lines = vec![
        muted_line("Review and edit the config Brain3 will write before startup begins."),
        blank_line(),
        field_line(
            "Vault path",
            &state.vault_path_input,
            f == SummaryField::VaultPath,
        ),
        field_line(
            "Username",
            &state.username_input,
            f == SummaryField::Username,
        ),
        field_line(
            "Client ID",
            &state.client_id_input,
            f == SummaryField::ClientId,
        ),
        field_badge_line(
            "Password mode",
            if state.generate_password {
                badge_span("Auto-generated", Color::Green)
            } else {
                badge_span("Custom password", Color::Cyan)
            },
            f == SummaryField::PasswordMode,
        ),
    ];

    if !state.generate_password {
        lines.push(field_line(
            "Password",
            &state.password_input,
            f == SummaryField::PasswordValue,
        ));
    }

    lines.extend([
        field_line(
            "Gateway port",
            &state.gateway_port_input,
            f == SummaryField::GatewayPort,
        ),
        field_line(
            "Container host port",
            &state.container_host_port_input,
            f == SummaryField::ContainerHostPort,
        ),
        field_line(
            "Container MCP port",
            &state.container_mcp_port_input,
            f == SummaryField::ContainerMcpPort,
        ),
        field_line(
            "Access token lifetime (secs)",
            &state.access_token_lifetime_secs_input,
            f == SummaryField::AccessTokenLifetimeSecs,
        ),
        field_line(
            "Refresh token lifetime (secs)",
            &state.refresh_token_lifetime_secs_input,
            f == SummaryField::RefreshTokenLifetimeSecs,
        ),
        field_badge_line(
            "PKCE required",
            if state.draft.pkce_required {
                badge_span("Enabled", Color::Green)
            } else {
                badge_span("Disabled", Color::Yellow)
            },
            f == SummaryField::PkceRequired,
        ),
        field_badge_line(
            "Hostname check",
            if state.draft.enforce_hostname_check {
                badge_span("Enabled", Color::Green)
            } else {
                badge_span("Disabled", Color::Yellow)
            },
            f == SummaryField::HostnameCheck,
        ),
        field_badge_line(
            "Internal-only container networking",
            if state.draft.container_network_isolated {
                badge_span("Enabled", Color::Green)
            } else {
                badge_span("Disabled", Color::Yellow)
            },
            f == SummaryField::ContainerNetworkIsolation,
        ),
        key_value_line(
            "Container runtime",
            format_container_runtime(state.draft.container_runtime),
        ),
        key_value_line("Container image", state.draft.container_image.clone()),
        key_value_line("Tunnel", format_tunnel_mode(&state.draft.tunnel_mode)),
        key_value_line(
            "Env file",
            state.preparation.paths.env_file.display().to_string(),
        ),
        key_value_line("Logs", state.log_file.display().to_string()),
    ]);

    lines
}

fn connection_card_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let Some(card) = &state.connection_card else {
        return vec![Line::from(Span::styled(
            "Connection card unavailable.",
            warning_style(),
        ))];
    };

    vec![
        Line::from(Span::styled(
            "Brain3 is configured and running.",
            success_style(),
        )),
        blank_line(),
        Line::from(Span::styled(
            "MCP Connection Details for AI app - See README for instructions",
            connection_heading_style(),
        )),
        blank_line(),
        key_value_line("Server URL", format!("{}/mcp", card.server_url)),
        key_value_line("Client ID", card.client_id.clone()),
        key_value_line("Client Secret", card.client_secret.clone()),
        key_value_line("Username", card.username.clone()),
        key_value_line("Password", card.password.clone()),
    ]
}

fn runtime_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let mut lines = vec![muted_line(
        "This screen shows live runtime status for Brain3.",
    )];

    if state.connection_card.is_some() {
        lines.push(muted_line("Press c to switch back to MCP config settings."));
    }

    lines.push(muted_line("Press l to toggle the logs view."));

    lines.push(blank_line());

    if state.startup_rx.is_some() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", spinner_char(state.tick_count)),
                accent_style(),
            ),
            Span::styled("Starting Brain3, please wait...", accent_style()),
        ]));
        return lines;
    }

    if let Some(runtime) = &state.runtime {
        lines.push(key_badge_line(
            "Container",
            startup_badge(&runtime.container_status),
        ));
        lines.push(key_badge_line(
            "Tunnel",
            startup_badge(&runtime.tunnel_status),
        ));

        if let Some(container) = runtime.config.container.as_ref() {
            lines.push(key_value_line(
                "Container runtime",
                format_container_runtime(container.runtime).to_string(),
            ));
            lines.push(key_value_line("Container image", container.image.clone()));
            let network = if container.isolation_strategy.is_some() {
                container.network_name.clone()
            } else {
                "bridge".to_string()
            };
            lines.push(key_value_line("Container network", network));
            lines.push(key_value_line(
                "Vault path",
                container.vault_path.display().to_string(),
            ));
        }

        if let Some(url) = &runtime.public_url {
            lines.push(key_value_line("Public URL", url.clone()));
        }

        if let Some(summary) = runtime.container_status.failure_summary() {
            lines.push(key_value_line("Container error", summary.to_string()));
        }

        if let Some(summary) = runtime.tunnel_status.failure_summary() {
            lines.push(key_value_line("Tunnel error", summary.to_string()));
        }

        lines.push(key_value_line(
            "Config",
            runtime.launch_plan.env_file.display().to_string(),
        ));
        lines.push(key_value_line(
            "Logs",
            runtime.launch_plan.log_file.display().to_string(),
        ));
        lines.push(blank_line());
    }

    match state.gateway_status() {
        GatewayServerStatus::NotStarted => {
            lines.push(key_badge_line(
                "Gateway",
                badge_span("Not started", Color::Yellow),
            ));
            if let Some(runtime) = &state.runtime {
                if let Some(summary) = runtime.primary_failure_summary() {
                    lines.push(key_value_line("Gateway status", summary.to_string()));
                }
            }
        }
        GatewayServerStatus::Running {
            bind_addr,
            local_url,
        } => {
            lines.push(key_badge_line(
                "Gateway",
                badge_span("Running", Color::Green),
            ));
            lines.push(key_value_line("Bind address", bind_addr.to_string()));
            lines.push(key_value_line("Local URL", local_url));
        }
        GatewayServerStatus::Stopped { bind_addr } => {
            lines.push(key_badge_line("Gateway", badge_span("Stopped", Color::Red)));
            lines.push(key_value_line("Bind address", bind_addr.to_string()));
        }
    }

    lines
}

fn status_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    if let Some(error) = &state.error_message {
        return vec![Line::from(vec![
            Span::styled("Error: ", error_style()),
            Span::styled(error.clone(), error_style()),
        ])];
    }

    if let Some(info) = &state.info_message {
        let has_active_task = state.startup_rx.is_some() || state.probe_rx.is_some();
        return vec![Line::from(if has_active_task {
            vec![
                Span::styled(
                    format!("{} ", spinner_char(state.tick_count)),
                    accent_style(),
                ),
                Span::styled(info.clone(), success_style()),
            ]
        } else {
            vec![
                Span::styled("Info: ", success_style()),
                Span::styled(info.clone(), success_style()),
            ]
        })];
    }

    vec![muted_line("Ready.")]
}

fn spinner_char(tick_count: u64) -> char {
    const FRAMES: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
    FRAMES
        .chars()
        .nth(tick_count as usize % FRAMES.chars().count())
        .unwrap_or('⠋')
}

fn action_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    match state.step {
        SetupStep::Welcome => continue_action_lines(vec![("[q]", "Quit")]),
        SetupStep::DependencyDoctor => dependency_action_lines(state),
        SetupStep::VaultPath => continue_action_lines(vec![
            ("[Type]", "Edit path"),
            ("[Backspace]", "Delete"),
            ("[Esc]", "Back"),
            ("[q]", "Quit"),
        ]),
        SetupStep::Auth => continue_action_lines(vec![
            ("[Tab/Up/Down]", "Move"),
            ("[Type]", "Edit field"),
            ("[g]", "Custom password"),
            ("[Esc]", "Back"),
            ("[q]", "Quit"),
        ]),
        SetupStep::PortsAndSettings => continue_action_lines(vec![
            ("[Tab/Up/Down]", "Move"),
            ("[Type]", "Edit port"),
            ("[t]", "Toggle setting"),
            ("[Esc]", "Back"),
            ("[q]", "Quit"),
        ]),
        SetupStep::Summary => vec![
            primary_action_line("Edit any field, then save config and start."),
            hint_line(vec![
                ("[Tab/↑↓]", "Move"),
                ("[Type]", "Edit"),
                ("[Space/t]", "Toggle"),
                ("[Esc]", "Back"),
                ("[q]", "Quit"),
                ("[Enter]", "Save & Start"),
            ]),
        ],
        SetupStep::ConnectionCard => vec![
            primary_action_line("Open runtime status when you're ready."),
            hint_line(vec![("[q]", "Quit"), ("[Enter]", "Open runtime status")]),
        ],
        SetupStep::RuntimeStatus => runtime_action_lines(state),
    }
}

fn runtime_action_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    match state.runtime_view {
        RuntimeView::Status => {
            let mut hints = vec![("[l]", "Logs")];
            if matches!(state.previous_step(), Some(SetupStep::ConnectionCard)) {
                hints.push(("[c]", "MCP config settings"));
            }
            if state.startup_rx.is_none() && state.probe_rx.is_none() && state.runtime.is_some() {
                hints.push(("[r]", "Refresh"));
            }
            hints.push(("[q]", "Quit"));

            let message = if state.startup_rx.is_some() {
                "Brain3 is starting..."
            } else if state
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.primary_failure_summary())
                .is_some()
            {
                "Brain3 startup failed. Review the runtime status and logs."
            } else {
                "Brain3 is running."
            };

            vec![primary_action_line(message), hint_line(hints)]
        }
        RuntimeView::Logs => {
            let mut hints = vec![
                ("[l]", "Status"),
                ("[Up/Down]", "Scroll"),
                ("[PgUp/PgDn]", "Page"),
                ("[End]", "Live"),
            ];
            if matches!(state.previous_step(), Some(SetupStep::ConnectionCard)) {
                hints.push(("[c]", "MCP config settings"));
            }
            hints.push(("[q]", "Quit"));

            vec![
                primary_action_line("Viewing runtime logs."),
                hint_line(hints),
            ]
        }
    }
}

fn continue_action_lines(extra_hints: Vec<(&str, &str)>) -> Vec<Line<'static>> {
    let mut hints = extra_hints;
    hints.push(("[Enter]", "Continue"));

    vec![
        primary_action_line("Continue when you're ready."),
        hint_line(hints),
    ]
}

fn dependency_action_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let actions = state.dependency_actions();
    if actions.is_empty() {
        return continue_action_lines(vec![("[r]", "Refresh"), ("[Esc]", "Back"), ("[q]", "Quit")]);
    }

    match state.dependency_focus {
        DependencyDoctorFocus::InstallAction => {
            let action_label = state
                .selected_dependency_action()
                .map(install_action_label)
                .unwrap_or("Install selected dependency");
            vec![
                primary_action_line(format!(
                    "Run the selected action when you're ready: {action_label}."
                )),
                hint_line(vec![
                    ("[Tab]", "Focus continue"),
                    ("[Up/Down]", "Select action"),
                    ("[r]", "Refresh"),
                    ("[Esc]", "Back"),
                    ("[q]", "Quit"),
                    ("[Enter]", "Run selected action"),
                ]),
            ]
        }
        DependencyDoctorFocus::Continue => continue_action_lines(vec![
            ("[Tab]", "Focus install actions"),
            ("[r]", "Refresh"),
            ("[Esc]", "Back"),
            ("[q]", "Quit"),
        ]),
    }
}

fn key_value_line(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        label_span(label),
        Span::raw(": "),
        Span::styled(value.into(), value_style()),
    ])
}

fn key_badge_line(label: &str, badge: Span<'static>) -> Line<'static> {
    Line::from(vec![label_span(label), Span::raw(": "), badge])
}

fn field_line(label: &str, value: &str, active: bool) -> Line<'static> {
    let pointer = if active {
        Span::styled("▶ ", accent_style())
    } else {
        Span::styled("  ", muted_style())
    };
    let label_style = if active {
        accent_style()
    } else {
        label_style()
    };
    let (display_value, value_style) = if active && value.is_empty() {
        (
            "[start typing here]".to_string(),
            accent_style().add_modifier(Modifier::UNDERLINED),
        )
    } else if active {
        (
            value.to_string(),
            value_style().add_modifier(Modifier::UNDERLINED),
        )
    } else {
        (value.to_string(), value_style())
    };

    Line::from(vec![
        pointer,
        Span::styled(format!("{label}: "), label_style),
        Span::styled(display_value, value_style),
    ])
}

fn field_badge_line(label: &str, badge: Span<'static>, active: bool) -> Line<'static> {
    let pointer = if active {
        Span::styled("▶ ", accent_style())
    } else {
        Span::styled("  ", muted_style())
    };
    let label_style = if active {
        accent_style()
    } else {
        label_style()
    };
    Line::from(vec![
        pointer,
        Span::styled(format!("{label}: "), label_style),
        badge,
    ])
}

fn hint_line(hints: Vec<(&str, &str)>) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, description)) in hints.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("    ", muted_style()));
        }
        spans.push(Span::styled(key.to_string(), accent_style()));
        spans.push(Span::styled(format!(" {description}"), muted_style()));
    }
    Line::from(spans)
}

fn primary_action_line(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), accent_style()))
}

fn muted_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), muted_style()))
}

fn blank_line() -> Line<'static> {
    Line::from("")
}

fn dependency_badge(availability: DependencyAvailability) -> Span<'static> {
    match availability {
        DependencyAvailability::Installed => badge_span("Installed", Color::Green),
        DependencyAvailability::InstallAvailable(_) => {
            badge_span("Install available", Color::Yellow)
        }
        DependencyAvailability::ManualInstallRequired => badge_span("Manual setup", Color::Red),
    }
}

fn bool_badge(installed: bool) -> Span<'static> {
    if installed {
        badge_span("Installed", Color::Green)
    } else {
        badge_span("Missing", Color::Yellow)
    }
}

fn optional_bool_badge(installed: Option<bool>) -> Span<'static> {
    match installed {
        Some(value) => bool_badge(value),
        None => Span::styled("n/a", muted_style()),
    }
}

fn startup_badge(status: &StartupStatus) -> Span<'static> {
    match status {
        StartupStatus::NotConfigured => badge_span("Not configured", Color::Yellow),
        StartupStatus::Ready => badge_span("Ready", Color::Green),
        StartupStatus::Failed { .. } => badge_span("Failed", Color::Red),
    }
}

fn badge_span(text: &str, color: Color) -> Span<'static> {
    let foreground = match color {
        Color::Red | Color::Blue => Color::White,
        _ => Color::Black,
    };

    Span::styled(
        format!(" {text} "),
        Style::default()
            .fg(foreground)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn format_package_manager(package_manager: Option<PackageManager>) -> &'static str {
    match package_manager {
        Some(PackageManager::Homebrew) => "Homebrew",
        Some(PackageManager::Apt) => "Apt",
        None => "n/a",
    }
}

fn format_container_runtime(runtime: ContainerRuntime) -> &'static str {
    match runtime {
        ContainerRuntime::Docker => "Docker",
        ContainerRuntime::MacOSContainer => "macOS container",
    }
}

fn format_tunnel_mode(mode: &TunnelModeDraft) -> String {
    match mode {
        TunnelModeDraft::CloudflareQuick => "Cloudflare quick tunnel".into(),
        TunnelModeDraft::CloudflareNamed {
            tunnel_name,
            domain,
        } => format!("Cloudflare named tunnel ({tunnel_name}.{domain})"),
        TunnelModeDraft::DirectPublicOrigin { hostname } => {
            format!("Direct public origin ({hostname})")
        }
    }
}

fn panel_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(border_style())
        .title(Line::from(Span::styled(
            format!(" {title} "),
            title_style(),
        )))
}

fn title_style() -> Style {
    Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::BOLD)
}

fn heading_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn section_heading_style() -> Style {
    Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::BOLD)
}

fn label_style() -> Style {
    Style::default().fg(Color::Blue)
}

fn label_span(label: &str) -> Span<'static> {
    Span::styled(label.to_string(), label_style())
}

fn value_style() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn accent_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn connection_heading_style() -> Style {
    Style::default()
        .fg(Color::Rgb(255, 190, 120))
        .add_modifier(Modifier::BOLD)
}

fn success_style() -> Style {
    Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD)
}

fn warning_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

fn error_style() -> Style {
    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
}

fn muted_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn border_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use brain3_core::application::first_run_setup::CURRENT_RELEASE;
    use brain3_core::domain::model::{
        ContainerRuntime, GatewayConfig, HostnameValidationConfig, MCPReverseProxyConfig,
        OAuthConfig,
    };
    use brain3_core::domain::setup::{
        DependencyAvailability, DependencyStatus, PackageManager, RuntimeLaunchPlan,
        SetupDraftConfig, SetupOperatingSystem, SetupPaths, SetupPreparation, SetupStep,
        TunnelModeDraft,
    };
    use brain3_platform::runtime::{RuntimeBootstrap, StartupStatus};

    use super::*;

    #[test]
    fn header_mentions_brain3_version() {
        let state = sample_state();
        let text = header_line(&state).to_string();

        assert!(text.contains("Brain3 v"));
        assert!(text.contains(release::APP_VERSION));
    }

    #[test]
    fn runtime_screen_shows_failed_container_status() {
        let state = sample_failed_runtime_state();
        let text = runtime_lines(&state)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Container:  Failed"));
        assert!(text.contains(&format!(
            "Container image: ghcr.io/tleyden/brain3-mcp-vault-tools:{CURRENT_RELEASE}"
        )));
        assert!(text.contains("Container runtime: Docker"));
        assert!(text.contains("Vault path: /missing/vault"));
        assert!(text.contains("Vault path does not exist"));
        assert!(text.contains("Gateway:  Not started"));
    }

    fn sample_state() -> FirstRunTuiState {
        FirstRunTuiState::new(
            "127.0.0.1".into(),
            PathBuf::from("/tmp/brain3.log"),
            SetupPreparation {
                paths: SetupPaths::new(
                    PathBuf::from("/tmp/brain3-home"),
                    PathBuf::from("/tmp/brain3-home/.env"),
                    PathBuf::from("/tmp/brain3-home/cloudflared"),
                ),
                draft: SetupDraftConfig {
                    gateway_port: 8421,
                    client_id: "brain3-oauth2-client".into(),
                    client_secret: "secret".into(),
                    access_token_lifetime_secs: 3600,
                    refresh_token_lifetime_secs: 90 * 24 * 60 * 60,
                    username: "admin".into(),
                    password: String::new(),
                    tunnel_mode: TunnelModeDraft::CloudflareQuick,
                    container_runtime: ContainerRuntime::MacOSContainer,
                    vault_path: PathBuf::from("/tmp/vault"),
                    container_image: release::default_container_image(),
                    container_host_port: 8420,
                    container_mcp_port: 8420,
                    container_network_isolated: true,
                    pkce_required: true,
                    enforce_hostname_check: true,
                    direct_public_origin_hostname: None,
                },
                dependencies: DependencyStatus {
                    operating_system: SetupOperatingSystem::MacOS,
                    package_manager: Some(PackageManager::Homebrew),
                    cloudflared: DependencyAvailability::Installed,
                    preferred_container_runtime: DependencyAvailability::Installed,
                    docker_installed: true,
                    macos_container_installed: Some(true),
                    homebrew_installed: Some(true),
                },
            },
        )
    }

    fn sample_failed_runtime_state() -> FirstRunTuiState {
        let mut state = sample_state();
        state.step = SetupStep::RuntimeStatus;
        state.runtime = Some(RuntimeBootstrap::new(
            Arc::new(GatewayConfig {
                port: 8421,
                host: "127.0.0.1".into(),
                token_db_path: PathBuf::from("/tmp/brain3-home/brain3.db"),
                oauth: OAuthConfig {
                    client_id: "brain3-oauth2-client".into(),
                    client_secret: "secret".into(),
                    access_token_lifetime_secs: 3600,
                    refresh_token_lifetime_secs: 90 * 24 * 60 * 60,
                    pkce_required: true,
                    username: "admin".into(),
                    password: "password".into(),
                },
                mcp_reverse_proxy: MCPReverseProxyConfig {
                    mcp_upstream_url: "http://127.0.0.1:8420".into(),
                    upstream_secret_file: PathBuf::from("/tmp/upstream_secret"),
                },
                hostname_validation: HostnameValidationConfig {
                    expected_host: None,
                    enforce: true,
                },
                container: Some(brain3_core::domain::model::ContainerStartupConfig {
                    runtime: ContainerRuntime::Docker,
                    image: format!(
                        "ghcr.io/tleyden/brain3-mcp-vault-tools:{CURRENT_RELEASE}"
                    ),
                    container_name: "brain3-mcp-vault-tools".into(),
                    vault_path: PathBuf::from("/missing/vault"),
                    upstream_secret_dir: PathBuf::from("/tmp"),
                    host_port: 8420,
                    container_port: 8420,
                    isolation_strategy: Some(brain3_core::domain::model::ContainerNetworkIsolationStrategy::DiscoverContainerIp),
                    network_name: "brain3-mcp-net".into(),
                    dev_mount_source: None,
                }),
                tunnel: None,
            }),
            "secret".into(),
            RuntimeLaunchPlan {
                paths: SetupPaths::new(
                    PathBuf::from("/tmp/brain3-home"),
                    PathBuf::from("/tmp/brain3-home/.env"),
                    PathBuf::from("/tmp/brain3-home/cloudflared"),
                ),
                env_file: PathBuf::from("/tmp/brain3-home/.env"),
                log_file: PathBuf::from("/tmp/brain3.log"),
            },
            None,
            StartupStatus::Failed {
                summary: "Vault path does not exist: /Obsidian/MyVault".into(),
            },
            StartupStatus::NotConfigured,
        ));
        state.error_message = Some("Vault path does not exist: /Obsidian/MyVault".into());
        state
    }
}
