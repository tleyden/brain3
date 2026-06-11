use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use brain3_core::domain::model::ContainerRuntime;
use brain3_core::domain::setup::{DependencyAvailability, PackageManager, SetupStep};
use brain3_platform::runtime::StartupStatus;

use crate::server::GatewayServerStatus;

use super::state::{install_action_label, AuthField, FirstRunTuiState};

pub fn draw(f: &mut ratatui::Frame, state: &FirstRunTuiState) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(4),
            Constraint::Length(3),
        ])
        .split(area);

    let header = Paragraph::new(format!("Brain3 Gateway  ·  {}", screen_title(state.step)))
        .block(Block::default().borders(Borders::ALL).title(" brain3 "))
        .style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(header, chunks[0]);

    let body = Paragraph::new(body_lines(state))
        .block(Block::default().borders(Borders::ALL).title(" Details "))
        .wrap(Wrap { trim: false });
    f.render_widget(body, chunks[1]);

    let status = Paragraph::new(status_lines(state))
        .block(Block::default().borders(Borders::ALL).title(" Status "))
        .wrap(Wrap { trim: false });
    f.render_widget(status, chunks[2]);

    let footer = Paragraph::new(help_lines(state))
        .block(Block::default().borders(Borders::ALL).title(" Controls "));
    f.render_widget(footer, chunks[3]);
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
        Line::from("This guided setup writes the default Brain3 config and starts the gateway."),
        Line::from("The same shell is also used later for everyday runtime status."),
        Line::from(""),
        Line::from(format!(
            "App home: {}",
            state.preparation.paths.app_home.display()
        )),
        Line::from(format!(
            "Env file: {}",
            state.preparation.paths.env_file.display()
        )),
        Line::from(format!("Logs: {}", state.log_file.display())),
    ]
}

fn dependency_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let deps = &state.preparation.dependencies;
    let mut lines = vec![
        Line::from(format!("Operating system: {:?}", deps.operating_system)),
        Line::from(format!(
            "Package manager: {}",
            format_package_manager(deps.package_manager)
        )),
        Line::from(format!(
            "cloudflared: {}",
            format_dependency(deps.cloudflared)
        )),
        Line::from(format!("Docker installed: {}", deps.docker_installed)),
        Line::from(format!(
            "macOS container installed: {}",
            deps.macos_container_installed
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".into())
        )),
        Line::from(format!(
            "Default runtime: {}",
            format_container_runtime(state.draft.container_runtime)
        )),
    ];

    let actions = state.dependency_actions();
    if !actions.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from("Installable actions:"));
        for (index, action) in actions.iter().enumerate() {
            lines.push(Line::from(format!(
                "  [{}] {}",
                index + 1,
                install_action_label(*action)
            )));
        }
    }

    lines
}

fn vault_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    vec![
        Line::from("Enter the absolute path to your Obsidian-compatible vault."),
        Line::from(""),
        Line::from(active_line("Vault path", &state.vault_path_input, true)),
    ]
}

fn auth_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let password_line = if state.generate_password {
        "auto-generated when writing config".to_string()
    } else {
        "*".repeat(state.password_input.len())
    };

    vec![
        Line::from("Client secret and access token are generated automatically."),
        Line::from(""),
        Line::from(active_line(
            "Username",
            &state.username_input,
            state.auth_focus == AuthField::Username,
        )),
        Line::from(active_line(
            "Client ID",
            &state.client_id_input,
            state.auth_focus == AuthField::ClientId,
        )),
        Line::from(active_line(
            "Password",
            &password_line,
            state.auth_focus == AuthField::Password && !state.generate_password,
        )),
        Line::from(format!(
            "Password mode: {}",
            if state.generate_password {
                "auto-generated"
            } else {
                "manual"
            }
        )),
    ]
}

fn summary_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    vec![
        Line::from("Review the config that will be written before startup begins."),
        Line::from(""),
        Line::from(format!("Vault path: {}", state.vault_path_input)),
        Line::from(format!("Username: {}", state.username_input)),
        Line::from(format!("Client ID: {}", state.client_id_input)),
        Line::from(format!(
            "Password mode: {}",
            if state.generate_password {
                "auto-generated"
            } else {
                "manual"
            }
        )),
        Line::from(format!(
            "Container runtime: {:?}",
            state.draft.container_runtime
        )),
        Line::from(format!("Tunnel mode: {:?}", state.draft.tunnel_mode)),
        Line::from(format!("Logs: {}", state.log_file.display())),
    ]
}

fn connection_card_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let Some(card) = &state.connection_card else {
        return vec![Line::from("Connection card unavailable.")];
    };

    vec![
        Line::from("Brain3 is configured and the gateway has started."),
        Line::from(""),
        Line::from(format!("Server URL: {}/mcp", card.server_url)),
        Line::from(format!("Client ID: {}", card.client_id)),
        Line::from(format!("Client Secret: {}", card.client_secret)),
        Line::from(format!("Username: {}", card.username)),
        Line::from(format!("Password: {}", card.password)),
        Line::from(format!("Logs: {}", card.log_file.display())),
    ]
}

fn runtime_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if let Some(runtime) = &state.runtime {
        lines.push(Line::from(format!(
            "Container: {}",
            format_startup_status(runtime.container_status)
        )));
        lines.push(Line::from(format!(
            "Tunnel: {}",
            match (&runtime.public_url, runtime.tunnel_status) {
                (Some(url), StartupStatus::Started) => format!("running ({url})"),
                (_, status) => format_startup_status(status).to_string(),
            }
        )));
        lines.push(Line::from(format!(
            "Logs: {}",
            runtime.launch_plan.log_file.display()
        )));
    }

    lines.push(Line::from(format!(
        "Gateway: {}",
        match state.gateway_status() {
            GatewayServerStatus::NotStarted => "not started".to_string(),
            GatewayServerStatus::Running {
                bind_addr,
                local_url,
            } => {
                format!("running on {bind_addr} ({local_url})")
            }
            GatewayServerStatus::Stopped { bind_addr } => format!("stopped ({bind_addr})"),
        }
    )));

    lines
}

fn status_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    if let Some(error) = &state.error_message {
        return vec![Line::from(Span::styled(
            error.clone(),
            Style::default().fg(Color::Red),
        ))];
    }

    if let Some(info) = &state.info_message {
        return vec![Line::from(Span::styled(
            info.clone(),
            Style::default().fg(Color::Green),
        ))];
    }

    vec![Line::from("Ready.")]
}

fn help_lines(state: &FirstRunTuiState) -> Vec<Line<'static>> {
    match state.step {
        SetupStep::Welcome => vec![Line::from("[Enter] continue    [q] quit")],
        SetupStep::DependencyDoctor => vec![Line::from(
            "[1-9] install action    [r] refresh    [Enter] continue    [Esc] back    [q] quit",
        )],
        SetupStep::VaultPath => vec![Line::from(
            "[Type] edit path    [Backspace] delete    [Enter] continue    [Esc] back    [q] quit",
        )],
        SetupStep::Auth => vec![Line::from(
            "[Type] edit field    [Tab] next field    [g] toggle password mode    [Enter] continue    [Esc] back    [q] quit",
        )],
        SetupStep::Summary => vec![Line::from(
            "[Enter] write config and start    [Esc] back    [q] quit",
        )],
        SetupStep::ConnectionCard => vec![Line::from("[Enter] runtime status    [q] quit")],
        SetupStep::RuntimeStatus => match state.previous_step() {
            Some(SetupStep::ConnectionCard) => {
                vec![Line::from("[c] MCP Config Settings    [q] quit")]
            }
            _ => vec![Line::from("[q] quit")],
        },
    }
}

fn active_line(label: &str, value: &str, active: bool) -> String {
    let marker = if active { ">" } else { " " };
    format!("{marker} {label}: {value}")
}

fn format_dependency(availability: DependencyAvailability) -> &'static str {
    match availability {
        DependencyAvailability::Installed => "installed",
        DependencyAvailability::InstallAvailable(_) => "install available",
        DependencyAvailability::ManualInstallRequired => "manual install required",
    }
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

fn format_startup_status(status: StartupStatus) -> &'static str {
    match status {
        StartupStatus::NotConfigured => "not configured",
        StartupStatus::Started => "started",
    }
}
