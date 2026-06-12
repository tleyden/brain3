use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use brain3_core::domain::model::ContainerRuntime;
use brain3_core::domain::setup::{
    DependencyAvailability, PackageManager, SetupStep, TunnelModeDraft,
};
use brain3_platform::runtime::StartupStatus;

use crate::server::GatewayServerStatus;

use super::state::{install_action_label, AuthField, DependencyDoctorFocus, FirstRunTuiState};

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

    let body = Paragraph::new(body_lines(state))
        .block(panel_block("Details"))
        .wrap(Wrap { trim: false });
    f.render_widget(body, chunks[2]);

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
        Span::styled("Brain3 Gateway", heading_style()),
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
        SetupStep::Summary => "Summary",
        SetupStep::ConnectionCard => "MCP Config Settings",
        SetupStep::RuntimeStatus => "Runtime Status",
    }
}

fn progress_caption(step: SetupStep) -> &'static str {
    match step {
        SetupStep::Welcome => "Start with the defaults Brain3 prepared for this machine.",
        SetupStep::DependencyDoctor => "Confirm local dependencies and install anything missing.",
        SetupStep::VaultPath => "Choose the Obsidian-compatible vault Brain3 should expose.",
        SetupStep::Auth => "Review the generated login defaults and customize them if needed.",
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
        SetupStep::Summary => 4,
        SetupStep::ConnectionCard | SetupStep::RuntimeStatus => 5,
    }
}

fn body_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    match state.step {
        SetupStep::Welcome => welcome_lines(state),
        SetupStep::DependencyDoctor => dependency_lines(state),
        SetupStep::VaultPath => vault_lines(state),
        SetupStep::Auth => auth_lines(state),
        SetupStep::Summary => summary_lines(state),
        SetupStep::ConnectionCard => connection_card_lines(state),
        SetupStep::RuntimeStatus => runtime_lines(state),
    }
}

fn welcome_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    vec![
        muted_line("This guided setup writes the default Brain3 config and starts the gateway."),
        muted_line("The same shell is also used later for everyday runtime status."),
        blank_line(),
        Line::from(Span::styled(
            "Here are the default locations for future reference.",
            accent_style(),
        )),
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
        muted_line("Start typing now. No extra key is required."),
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
        muted_line("Client secret and access token are generated automatically."),
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

fn summary_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    vec![
        muted_line("Review the config Brain3 will write before startup begins."),
        blank_line(),
        key_value_line("Vault path", state.vault_path_input.clone()),
        key_value_line("Username", state.username_input.clone()),
        key_value_line("Client ID", state.client_id_input.clone()),
        key_badge_line(
            "Password mode",
            if state.generate_password {
                badge_span("Auto-generated", Color::Green)
            } else {
                badge_span("Custom password", Color::Cyan)
            },
        ),
        key_value_line(
            "Container runtime",
            format_container_runtime(state.draft.container_runtime),
        ),
        key_value_line("Tunnel", format_tunnel_mode(&state.draft.tunnel_mode)),
        key_value_line(
            "Env file",
            state.preparation.paths.env_file.display().to_string(),
        ),
        key_value_line("Logs", state.log_file.display().to_string()),
    ]
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
        muted_line("Use these values in your AI app when it asks for MCP connection details."),
        blank_line(),
        key_value_line("Server URL", format!("{}/mcp", card.server_url)),
        key_value_line("Client ID", card.client_id.clone()),
        key_value_line("Client Secret", card.client_secret.clone()),
        key_value_line("Username", card.username.clone()),
        key_value_line("Password", card.password.clone()),
        key_value_line("Logs", card.log_file.display().to_string()),
    ]
}

fn runtime_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if let Some(runtime) = &state.runtime {
        lines.push(key_badge_line(
            "Container",
            startup_badge(runtime.container_status),
        ));
        lines.push(key_badge_line(
            "Tunnel",
            startup_badge(runtime.tunnel_status),
        ));

        if let Some(url) = &runtime.public_url {
            lines.push(key_value_line("Public URL", url.clone()));
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
        return vec![Line::from(vec![
            Span::styled("Info: ", success_style()),
            Span::styled(info.clone(), success_style()),
        ])];
    }

    vec![muted_line("Ready.")]
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
        SetupStep::Summary => vec![
            primary_action_line("Ready to launch Brain3."),
            muted_line("⚠ The UI will \"stick\" for 5s, but it's starting. Known issue."),
            hint_line(vec![
                ("[Esc]", "Back"),
                ("[q]", "Quit"),
                ("[Enter]", "Save Config and Start"),
            ]),
        ],
        SetupStep::ConnectionCard => vec![
            primary_action_line("Open runtime status when you're ready."),
            hint_line(vec![("[q]", "Quit"), ("[Enter]", "Open runtime status")]),
        ],
        SetupStep::RuntimeStatus => match state.previous_step() {
            Some(SetupStep::ConnectionCard) => vec![
                primary_action_line("Press c to view MCP config settings again."),
                hint_line(vec![("[c]", "MCP config settings"), ("[q]", "Quit")]),
            ],
            _ => vec![
                primary_action_line("Brain3 is running."),
                hint_line(vec![("[q]", "Quit")]),
            ],
        },
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

fn startup_badge(status: StartupStatus) -> Span<'static> {
    match status {
        StartupStatus::NotConfigured => badge_span("Not configured", Color::Yellow),
        StartupStatus::Started => badge_span("Started", Color::Green),
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
