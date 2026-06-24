use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::oneshot;

use brain3_core::application::first_run_setup::FirstRunSetupUseCase;
use brain3_core::domain::setup::{
    AccessModeDraft, ConnectionCard, FinalizeSetupRequest, RuntimeLaunchPlan, SetupDefaults,
    SetupStep,
};
use brain3_core::ports::setup_system::SetupSystemPort;
use brain3_platform::runtime::probe_mcp_vault_list;
use brain3_platform::setup::PlatformSetupSystem;

use crate::server;
use crate::server::ConfiguredGatewaySession;
use crate::{load_config, RuntimeOverrides};

use super::screens;
use super::state::{
    install_action_label, validate_port_input, validate_positive_u64_input, AuthField,
    DependencyDoctorFocus, FirstRunTuiState, PortsField, RuntimeView,
};

pub enum GatewayTuiLaunch {
    FirstRun,
    Configured { launch_plan: RuntimeLaunchPlan },
}

pub async fn run_gateway_tui(
    host: &str,
    log_file: PathBuf,
    launch: GatewayTuiLaunch,
    setup_defaults: SetupDefaults,
    runtime_overrides: RuntimeOverrides,
    brain3_home: Option<PathBuf>,
) -> Result<()> {
    let setup_system: Arc<dyn SetupSystemPort> = Arc::new(match brain3_home {
        Some(dir) => PlatformSetupSystem::with_home_override(dir),
        None => PlatformSetupSystem::new(),
    });
    let use_case = FirstRunSetupUseCase::new(Arc::clone(&setup_system), setup_defaults);

    // Start TUI immediately so the screen is live during all startup work.
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = match launch {
        GatewayTuiLaunch::FirstRun => FirstRunTuiState::new(
            host.to_string(),
            log_file.clone(),
            use_case
                .prepare()
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?,
        ),
        GatewayTuiLaunch::Configured { launch_plan } => {
            let config = load_config(launch_plan.env_file.clone(), &runtime_overrides)?;
            let mut preparation = use_case
                .prepare_from_existing_config(config.as_ref())
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            preparation.paths.env_file = launch_plan.env_file.clone();
            FirstRunTuiState::new_configured(host.to_string(), log_file.clone(), preparation)
        }
    };

    let result = event_loop(
        &mut terminal,
        &mut state,
        &use_case,
        setup_system,
        &runtime_overrides,
    )
    .await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    let cleanup_result = cleanup(&mut state).await;
    result.and(cleanup_result)
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut FirstRunTuiState,
    use_case: &FirstRunSetupUseCase,
    setup_system: Arc<dyn SetupSystemPort>,
    runtime_overrides: &RuntimeOverrides,
) -> Result<()> {
    loop {
        handle_runtime_tick(state);

        if let Some(rx) = &mut state.startup_rx {
            if let Ok(result) = rx.try_recv() {
                state.startup_rx = None;
                apply_startup_result(state, result, use_case);
            }
        }

        terminal.draw(|f| screens::draw(f, state))?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if key.code == KeyCode::Char('q') {
            return Ok(());
        }

        match state.step {
            SetupStep::Welcome => match key.code {
                KeyCode::Enter => {
                    state.clear_messages();
                    state.step = SetupStep::DependencyDoctor;
                }
                _ => {}
            },
            SetupStep::DependencyDoctor => match key.code {
                KeyCode::Esc => {
                    state.clear_messages();
                    state.step = SetupStep::Welcome;
                }
                KeyCode::Enter => {
                    state.clear_messages();
                    if matches!(state.dependency_focus, DependencyDoctorFocus::InstallAction) {
                        if let Some(action) = state.selected_dependency_action() {
                            run_install_action(state, Arc::clone(&setup_system), action).await;
                        } else {
                            state.step = SetupStep::VaultPath;
                        }
                    } else {
                        state.step = SetupStep::VaultPath;
                    }
                }
                KeyCode::Char('r') => {
                    refresh_dependencies(state, Arc::clone(&setup_system)).await;
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    state.toggle_dependency_focus();
                }
                KeyCode::Up => state.previous_dependency_action(),
                KeyCode::Down => state.next_dependency_action(),
                _ => {}
            },
            SetupStep::VaultPath => match key.code {
                KeyCode::Esc => {
                    state.clear_messages();
                    state.step = SetupStep::DependencyDoctor;
                }
                KeyCode::Enter => {
                    advance_from_vault_path(state, use_case).await;
                }
                KeyCode::Backspace => {
                    state.vault_path_input.pop();
                }
                KeyCode::Char(ch) => {
                    state.vault_path_input.push(ch);
                }
                _ => {}
            },
            SetupStep::AccessMode => match key.code {
                KeyCode::Esc => {
                    state.clear_messages();
                    state.step = SetupStep::VaultPath;
                }
                KeyCode::Up => state.previous_access_mode_focus(),
                KeyCode::Down => state.next_access_mode_focus(),
                KeyCode::Char(' ') => {}
                KeyCode::Enter => {
                    state.clear_messages();
                    state.step = match state.draft.access_mode {
                        AccessModeDraft::LocalOnly => SetupStep::PortsAndSettings,
                        AccessModeDraft::RemoteOnly | AccessModeDraft::Both => SetupStep::Auth,
                    };
                    if state.step == SetupStep::PortsAndSettings {
                        state.reset_ports_focus();
                    }
                }
                _ => {}
            },
            SetupStep::Auth => match key.code {
                KeyCode::Esc => {
                    state.clear_messages();
                    state.step = SetupStep::AccessMode;
                }
                KeyCode::Enter => {
                    state.clear_messages();
                    state.reset_ports_focus();
                    state.step = SetupStep::PortsAndSettings;
                }
                KeyCode::Tab | KeyCode::Down => {
                    state.next_auth_focus();
                }
                KeyCode::BackTab | KeyCode::Up => {
                    state.previous_auth_focus();
                }
                KeyCode::Char('g')
                    if !matches!(state.auth_focus, AuthField::Username | AuthField::ClientId) =>
                {
                    state.generate_password = !state.generate_password;
                    if state.generate_password && state.auth_focus == AuthField::Password {
                        state.auth_focus = AuthField::Username;
                    }
                }
                KeyCode::Backspace => match state.auth_focus {
                    AuthField::Username => {
                        state.username_input.pop();
                    }
                    AuthField::ClientId => {
                        state.client_id_input.pop();
                    }
                    AuthField::Password if !state.generate_password => {
                        state.password_input.pop();
                    }
                    AuthField::Password => {}
                },
                KeyCode::Char(ch) => match state.auth_focus {
                    AuthField::Username => state.username_input.push(ch),
                    AuthField::ClientId => state.client_id_input.push(ch),
                    AuthField::Password if !state.generate_password => {
                        state.password_input.push(ch)
                    }
                    AuthField::Password => {}
                },
                _ => {}
            },
            SetupStep::PortsAndSettings => match key.code {
                KeyCode::Esc => {
                    state.clear_messages();
                    state.step = match state.draft.access_mode {
                        AccessModeDraft::LocalOnly => SetupStep::AccessMode,
                        AccessModeDraft::RemoteOnly | AccessModeDraft::Both => SetupStep::Auth,
                    };
                }
                KeyCode::Enter => {
                    state.clear_messages();
                    if let Err(msg) =
                        validate_port_input(&state.local_mcp_port_input, "Local MCP port")
                    {
                        tracing::debug!(msg, "port validation failed");
                        state.error_message = Some(msg);
                    } else if state.draft.access_mode != AccessModeDraft::LocalOnly {
                        if let Err(msg) =
                            validate_port_input(&state.gateway_port_input, "Gateway port")
                        {
                            tracing::debug!(msg, "port validation failed");
                            state.error_message = Some(msg);
                        } else if let Err(msg) = validate_port_input(
                            &state.container_host_port_input,
                            "Container host port",
                        ) {
                            tracing::debug!(msg, "port validation failed");
                            state.error_message = Some(msg);
                        } else if let Err(msg) = validate_port_input(
                            &state.container_mcp_port_input,
                            "Container MCP port",
                        ) {
                            tracing::debug!(msg, "port validation failed");
                            state.error_message = Some(msg);
                        } else if let Err(msg) = validate_positive_u64_input(
                            &state.access_token_lifetime_secs_input,
                            "Access token lifetime",
                        ) {
                            tracing::debug!(msg, "lifetime validation failed");
                            state.error_message = Some(msg);
                        } else if let Err(msg) = validate_positive_u64_input(
                            &state.refresh_token_lifetime_secs_input,
                            "Refresh token lifetime",
                        ) {
                            tracing::debug!(msg, "lifetime validation failed");
                            state.error_message = Some(msg);
                        } else {
                            state.step = SetupStep::Summary;
                        }
                    } else if let Err(msg) =
                        validate_port_input(&state.container_host_port_input, "Container host port")
                    {
                        tracing::debug!(msg, "port validation failed");
                        state.error_message = Some(msg);
                    } else if let Err(msg) =
                        validate_port_input(&state.container_mcp_port_input, "Container MCP port")
                    {
                        tracing::debug!(msg, "port validation failed");
                        state.error_message = Some(msg);
                    } else {
                        state.step = SetupStep::Summary;
                    }
                }
                KeyCode::Tab | KeyCode::Down => {
                    let access_mode = state.draft.access_mode.clone();
                    state.next_ports_focus(&access_mode);
                }
                KeyCode::BackTab | KeyCode::Up => {
                    let access_mode = state.draft.access_mode.clone();
                    state.previous_ports_focus(&access_mode);
                }
                KeyCode::Char('t') => {
                    state.toggle_ports_boolean();
                }
                KeyCode::Backspace if state.ports_focus_is_text_field() => {
                    match state.ports_focus {
                        PortsField::GatewayPort => {
                            state.gateway_port_input.pop();
                        }
                        PortsField::LocalMcpPort => {
                            state.local_mcp_port_input.pop();
                        }
                        PortsField::ContainerHostPort => {
                            state.container_host_port_input.pop();
                        }
                        PortsField::ContainerMcpPort => {
                            state.container_mcp_port_input.pop();
                        }
                        PortsField::ContainerName => {
                            state.container_name_input.pop();
                        }
                        PortsField::AccessTokenLifetimeSecs => {
                            state.access_token_lifetime_secs_input.pop();
                        }
                        PortsField::RefreshTokenLifetimeSecs => {
                            state.refresh_token_lifetime_secs_input.pop();
                        }
                        PortsField::ContainerNetworkName => {
                            state.container_network_name_input.pop();
                        }
                        _ => {}
                    }
                }
                KeyCode::Char(ch)
                    if state.ports_focus_is_text_field()
                        && (!state.ports_focus_is_digits_only() || ch.is_ascii_digit()) =>
                {
                    match state.ports_focus {
                        PortsField::GatewayPort => state.gateway_port_input.push(ch),
                        PortsField::LocalMcpPort => state.local_mcp_port_input.push(ch),
                        PortsField::ContainerHostPort => state.container_host_port_input.push(ch),
                        PortsField::ContainerMcpPort => state.container_mcp_port_input.push(ch),
                        PortsField::ContainerName => state.container_name_input.push(ch),
                        PortsField::AccessTokenLifetimeSecs => {
                            state.access_token_lifetime_secs_input.push(ch)
                        }
                        PortsField::RefreshTokenLifetimeSecs => {
                            state.refresh_token_lifetime_secs_input.push(ch)
                        }
                        PortsField::ContainerNetworkName => {
                            state.container_network_name_input.push(ch)
                        }
                        _ => {}
                    }
                }
                _ => {}
            },
            SetupStep::Summary => match key.code {
                KeyCode::Esc => {
                    state.clear_messages();
                    state.step = SetupStep::PortsAndSettings;
                }
                KeyCode::Enter => {
                    finalize_and_start(state, use_case, runtime_overrides.clone()).await;
                }
                KeyCode::Tab | KeyCode::Down => {
                    state.next_summary_focus();
                }
                KeyCode::BackTab | KeyCode::Up => {
                    state.previous_summary_focus();
                }
                KeyCode::Char(' ') | KeyCode::Char('t') if !state.summary_focus_is_text_field() => {
                    state.toggle_summary_field();
                }
                KeyCode::Backspace if state.summary_focus_is_text_field() => {
                    state.summary_char_pop();
                }
                KeyCode::Char(ch) if state.summary_focus_is_text_field() => {
                    if state.summary_focus_is_digits_only() {
                        if ch.is_ascii_digit() {
                            state.summary_char_push(ch);
                        }
                    } else {
                        state.summary_char_push(ch);
                    }
                }
                _ => {}
            },
            SetupStep::ConnectionCard => match key.code {
                KeyCode::Enter => {
                    state.clear_messages();
                    state.show_runtime_status();
                    state.step = SetupStep::RuntimeStatus;
                }
                _ => {}
            },
            SetupStep::RuntimeStatus => match key.code {
                KeyCode::Char('l') => state.toggle_runtime_view(),
                KeyCode::Up if state.runtime_view == RuntimeView::Logs => state.scroll_logs_up(1),
                KeyCode::Down if state.runtime_view == RuntimeView::Logs => {
                    state.scroll_logs_down(1);
                }
                KeyCode::PageUp if state.runtime_view == RuntimeView::Logs => {
                    state.scroll_logs_up(runtime_logs_page_size());
                }
                KeyCode::PageDown if state.runtime_view == RuntimeView::Logs => {
                    state.scroll_logs_down(runtime_logs_page_size());
                }
                KeyCode::End if state.runtime_view == RuntimeView::Logs => {
                    state.jump_logs_to_end();
                }
                KeyCode::Char('c') if state.connection_card.is_some() => {
                    state.clear_messages();
                    state.step = SetupStep::ConnectionCard;
                }
                KeyCode::Char('r')
                    if state.runtime_view == RuntimeView::Status
                        && state.probe_rx.is_none()
                        && state.startup_rx.is_none() =>
                {
                    if let Some(runtime) = &state.runtime {
                        let url = runtime.config.mcp_reverse_proxy.mcp_upstream_url.clone();
                        let secret = runtime.upstream_secret.clone();
                        let (tx, rx) = oneshot::channel();
                        tokio::spawn(async move {
                            let _ = tx.send(probe_mcp_vault_list(&url, &secret).await);
                        });
                        state.probe_rx = Some(rx);
                        state.clear_messages();
                        state.info_message = Some("Checking MCP health...".into());
                    }
                }
                _ => {}
            },
        }
    }
}

fn handle_runtime_tick(state: &mut FirstRunTuiState) {
    state.tick_count = state.tick_count.wrapping_add(1);
    if matches!(state.step, SetupStep::RuntimeStatus) {
        state.refresh_runtime_logs();
    }
    if let Some(rx) = &mut state.probe_rx {
        match rx.try_recv() {
            Ok(Ok(())) => {
                state.probe_rx = None;
                state.info_message = Some("MCP health check passed.".into());
                state.error_message = None;
            }
            Ok(Err(e)) => {
                state.probe_rx = None;
                state.error_message = Some(format!("MCP health check failed: {e}"));
                state.info_message = None;
            }
            Err(_) => {}
        }
    }
}

fn runtime_logs_page_size() -> usize {
    crossterm::terminal::size()
        .map(|(_, rows)| rows.saturating_sub(20).max(1) as usize)
        .unwrap_or(10)
}

async fn advance_from_vault_path(state: &mut FirstRunTuiState, use_case: &FirstRunSetupUseCase) {
    state.clear_messages();
    let vault_path_input = state.vault_path_input.trim();
    let vault_path = std::path::PathBuf::from(vault_path_input);

    match use_case.validate_vault_path(&vault_path).await {
        Ok(()) => {
            state.vault_path_input = vault_path_input.to_string();
            state.step = SetupStep::AccessMode;
        }
        Err(error) => {
            tracing::error!(error = %error, "vault path validation failed");
            state.error_message = Some(error.to_string());
        }
    }
}

async fn refresh_dependencies(
    state: &mut FirstRunTuiState,
    setup_system: Arc<dyn SetupSystemPort>,
) {
    match setup_system.collect_dependency_status().await {
        Ok(dependencies) => {
            state.set_dependencies(dependencies);
            state.error_message = None;
            state.info_message = Some("Dependency status refreshed.".into());
        }
        Err(error) => {
            tracing::error!(error = %error, "failed to collect dependency status");
            state.error_message = Some(error.to_string());
            state.info_message = None;
        }
    }
}

async fn run_install_action(
    state: &mut FirstRunTuiState,
    setup_system: Arc<dyn SetupSystemPort>,
    action: brain3_core::domain::setup::InstallAction,
) {
    state.clear_messages();
    let action_label = install_action_label(action);
    state.info_message = Some(format!("Running {action_label}..."));
    match setup_system.run_install_action(action).await {
        Ok(()) => {
            state.info_message = Some(format!(
                "{action_label} completed. Refreshing dependency status."
            ));
            refresh_dependencies(state, setup_system).await;
        }
        Err(error) => {
            tracing::error!(error = %error, action = %action_label, "install action failed");
            state.error_message = Some(error.to_string());
            state.info_message = None;
        }
    }
}

async fn finalize_and_start(
    state: &mut FirstRunTuiState,
    use_case: &FirstRunSetupUseCase,
    runtime_overrides: RuntimeOverrides,
) {
    state.clear_messages();

    let request: FinalizeSetupRequest = state.apply_inputs_to_draft();

    // Fast: validate inputs, generate secrets, write env file.
    let summary = match use_case
        .finalize(request)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))
    {
        Ok(summary) => summary,
        Err(error) => {
            tracing::error!(error = %error, "failed to finalize setup");
            state.error_message = Some(error.to_string());
            return;
        }
    };

    // Slow: container startup, tunnel, gateway bind — run in background so TUI stays live.
    let launch_plan = RuntimeLaunchPlan {
        paths: summary.paths.clone(),
        env_file: summary.paths.env_file.clone(),
        log_file: state.log_file.clone(),
    };
    let host = state.host.clone();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ =
            tx.send(start_configured_runtime_session(&host, launch_plan, runtime_overrides).await);
    });

    state.summary = Some(summary);
    state.startup_rx = Some(rx);
    state.info_message = Some("Starting Brain3...".into());
    state.step = SetupStep::RuntimeStatus;
}

fn apply_startup_result(
    state: &mut FirstRunTuiState,
    result: anyhow::Result<ConfiguredGatewaySession>,
    use_case: &FirstRunSetupUseCase,
) {
    let session = match result {
        Err(error) => {
            tracing::error!(error = %error, "startup task failed");
            state.error_message = Some(error.to_string());
            state.info_message = None;
            return;
        }
        Ok(session) => session,
    };

    let ConfiguredGatewaySession {
        runtime,
        server,
        display_url,
    } = session;

    if let Some(display_url) = display_url {
        let card = if let Some(summary) = state.summary.as_ref() {
            // Wizard path: build card from the summary written to disk.
            tracing::debug!(server_url = %display_url, "building connection card after first-run wizard");
            use_case.build_connection_card(display_url, state.log_file.clone(), summary)
        } else {
            // Configured-launch path: credentials come from the loaded runtime config.
            let oauth = &runtime.config.oauth;
            tracing::trace!(
                server_url = %display_url,
                runtime_client_id = %oauth.client_id,
                runtime_username = %oauth.username,
                preparation_client_id = %state.preparation.draft.client_id,
                preparation_username = %state.preparation.draft.username,
                "building connection card: credentials from runtime config (loaded from disk)"
            );
            if oauth.client_id != state.preparation.draft.client_id
                || oauth.username != state.preparation.draft.username
            {
                tracing::warn!(
                    runtime_client_id = %oauth.client_id,
                    runtime_username = %oauth.username,
                    preparation_client_id = %state.preparation.draft.client_id,
                    preparation_username = %state.preparation.draft.username,
                    "connection card credentials differ from preparation draft — env file may \
                     have changed since startup"
                );
            }
            ConnectionCard {
                server_url: display_url,
                client_id: oauth.client_id.clone(),
                client_secret: oauth.client_secret.clone(),
                username: oauth.username.clone(),
                password: oauth.password.clone(),
                log_file: state.log_file.clone(),
            }
        };
        state.connection_card = Some(card);
    }

    state.runtime = Some(runtime);
    state.server = server;

    if let Some(runtime) = &state.runtime {
        if let Some(failure) = runtime.primary_failure_summary() {
            tracing::error!(failure, "runtime reported primary failure");
            state.error_message = Some(failure.to_string());
            state.info_message = None;
            if state.summary.is_some() {
                state.step = SetupStep::Summary;
            }
        } else {
            state.info_message = Some("Brain3 is running.".into());
            // Wizard path shows the connection card first so the user can copy credentials.
            // Configured-launch path goes straight to RuntimeStatus (credentials already known).
            if state.summary.is_some() {
                state.step = if state.connection_card.is_some() {
                    SetupStep::ConnectionCard
                } else {
                    SetupStep::RuntimeStatus
                };
            }
        }
    }
}

async fn start_configured_runtime_session(
    host: &str,
    launch_plan: RuntimeLaunchPlan,
    runtime_overrides: RuntimeOverrides,
) -> Result<ConfiguredGatewaySession> {
    server::spawn_configured_gateway_session(host, launch_plan, runtime_overrides).await
}

async fn cleanup(state: &mut FirstRunTuiState) -> Result<()> {
    if let Some(server) = state.server.take() {
        server.shutdown().await?;
    }
    if let Some(mut runtime) = state.runtime.take() {
        runtime.shutdown_managed_runtime().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use brain3_core::application::first_run_setup::FirstRunSetupUseCase;
    use brain3_core::domain::model::{
        AccessMode, ContainerRuntime, GatewayConfig, HostnameValidationConfig, MCPReverseProxyConfig,
        OAuthConfig,
    };
    use brain3_core::domain::setup::{
        AccessModeDraft, DependencyAvailability, DependencyStatus, SetupDefaults, SetupDraftConfig,
        SetupOperatingSystem, SetupPaths, SetupPreparation, SetupStep, TunnelModeDraft,
    };
    use brain3_platform::runtime::{RuntimeBootstrap, StartupStatus};
    use brain3_platform::setup::PlatformSetupSystem;

    use crate::server::ConfiguredGatewaySession;

    use super::*;

    #[test]
    fn failed_startup_returns_user_to_summary_for_retry() {
        let use_case = FirstRunSetupUseCase::new(
            Arc::new(PlatformSetupSystem::with_environment(
                SetupOperatingSystem::Linux,
                None,
            )),
            SetupDefaults {
                default_container_image_repo: "ghcr.io/tleyden/brain3-mcp-vault-tools".into(),
            },
        );
        let mut state = FirstRunTuiState::new_configured(
            "127.0.0.1".into(),
            PathBuf::from("/tmp/brain3.log"),
            sample_preparation(),
        );
        let paths = state.preparation.paths.clone();
        state.summary = Some(brain3_core::domain::setup::SetupSummary {
            paths: paths.clone(),
            draft: state.draft.clone(),
            dependencies: state.preparation.dependencies.clone(),
        });
        state.step = SetupStep::RuntimeStatus;

        apply_startup_result(
            &mut state,
            Ok(ConfiguredGatewaySession {
                runtime: RuntimeBootstrap::new(
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
                            upstream_secret: "secret".into(),
                        },
                        hostname_validation: HostnameValidationConfig {
                            expected_host: None,
                            enforce: true,
                        },
                        access_mode: AccessMode::Both,
                        local_mcp: None,
                        container: Some(brain3_core::domain::model::ContainerStartupConfig {
                            runtime: ContainerRuntime::Docker,
                            image: "ghcr.io/tleyden/brain3-mcp-vault-tools:v0.2.3".into(),
                            container_name: "brain3-mcp-vault-tools".into(),
                            network_name: "brain3-mcp-net".into(),
                            vault_path: PathBuf::from("/tmp/vault"),
                            upstream_secret: "secret".into(),
                            host_port: 8420,
                            container_port: 8420,
                            isolation_strategy: None,
                            dev_mount_source: None,
                            mcp_log_level: None,
                        }),
                        tunnel: None,
                    }),
                    "secret".into(),
                    brain3_core::domain::setup::RuntimeLaunchPlan {
                        paths: paths.clone(),
                        env_file: paths.env_file.clone(),
                        log_file: PathBuf::from("/tmp/brain3.log"),
                    },
                    None,
                    StartupStatus::Failed {
                        summary: "container name 'brain3-mcp-vault-tools' already exists; choose a different container name".into(),
                    },
                    StartupStatus::NotConfigured,
                    false,
                ),
                server: None,
                display_url: None,
            }),
            &use_case,
        );

        assert_eq!(state.step, SetupStep::Summary);
        assert!(state
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("different container name"));
    }

    fn sample_preparation() -> SetupPreparation {
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
                password: "password".into(),
                access_mode: AccessModeDraft::Both,
                tunnel_mode: TunnelModeDraft::CloudflareQuick,
                container_runtime: ContainerRuntime::MacOSContainer,
                vault_path: PathBuf::from("/tmp/vault"),
                container_image_repo: "ghcr.io/tleyden/brain3-mcp-vault-tools".into(),
                container_host_port: 8420,
                container_mcp_port: 8420,
                container_name: "brain3-mcp-vault-tools".into(),
                container_network_isolated: true,
                container_network_name: "brain3-mcp-net".into(),
                local_mcp_enabled: true,
                local_mcp_port: 8422,
                local_mcp_bearer_token: "local-token".into(),
                pkce_required: true,
                enforce_hostname_check: true,
                direct_public_origin_hostname: None,
            },
            dependencies: DependencyStatus {
                operating_system: SetupOperatingSystem::MacOS,
                package_manager: None,
                cloudflared: DependencyAvailability::Installed,
                preferred_container_runtime: DependencyAvailability::Installed,
                docker_installed: true,
                macos_container_installed: Some(true),
                homebrew_installed: Some(true),
            },
        }
    }
}
