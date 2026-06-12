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

use brain3_core::application::first_run_setup::FirstRunSetupUseCase;
use brain3_core::domain::setup::{
    FinalizeSetupRequest, RuntimeLaunchPlan, SetupDefaults, SetupStep,
};
use brain3_core::ports::setup_system::SetupSystemPort;
use brain3_platform::setup::PlatformSetupSystem;

use crate::server;
use crate::server::ConfiguredGatewaySession;
use crate::RuntimeOverrides;

use super::screens;
use super::state::{
    install_action_label, validate_port_input, AuthField, DependencyDoctorFocus,
    FirstRunTuiState, PortsField, RuntimeView,
};

pub enum GatewayTuiLaunch {
    FirstRun,
    Configured { launch_plan: RuntimeLaunchPlan },
}

pub async fn run_gateway_tui(
    host: &str,
    log_file: std::path::PathBuf,
    launch: GatewayTuiLaunch,
    setup_defaults: SetupDefaults,
    runtime_overrides: RuntimeOverrides,
) -> Result<()> {
    let setup_system: Arc<dyn SetupSystemPort> = Arc::new(PlatformSetupSystem::new());
    let use_case = FirstRunSetupUseCase::new(Arc::clone(&setup_system), setup_defaults);
    let preparation = use_case
        .prepare()
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;

    let mut state = match launch {
        GatewayTuiLaunch::FirstRun => {
            FirstRunTuiState::new(host.to_string(), log_file.clone(), preparation)
        }
        GatewayTuiLaunch::Configured { launch_plan } => {
            let session = server::spawn_configured_gateway_session(
                host,
                launch_plan,
                runtime_overrides.clone(),
            )
            .await?;
            tracing::debug!(
                server_url = ?session.display_url,
                "building connection card for configured launch"
            );
            FirstRunTuiState::new_runtime(
                host.to_string(),
                log_file.clone(),
                preparation,
                session.display_url,
                session.runtime,
                session.server,
            )
        }
    };

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut state, &use_case, setup_system).await;

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
) -> Result<()> {
    loop {
        handle_runtime_tick(state);
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
            SetupStep::Auth => match key.code {
                KeyCode::Esc => {
                    state.clear_messages();
                    state.step = SetupStep::VaultPath;
                }
                KeyCode::Enter => {
                    state.clear_messages();
                    state.step = SetupStep::PortsAndSettings;
                }
                KeyCode::Tab | KeyCode::Down => {
                    state.next_auth_focus();
                }
                KeyCode::BackTab | KeyCode::Up => {
                    state.previous_auth_focus();
                }
                KeyCode::Char('g') => {
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
                    state.step = SetupStep::Auth;
                }
                KeyCode::Enter => {
                    state.clear_messages();
                    if let Err(msg) = validate_port_input(
                        &state.gateway_port_input,
                        "Gateway port",
                    ) {
                        state.error_message = Some(msg);
                    } else if let Err(msg) = validate_port_input(
                        &state.container_host_port_input,
                        "Container host port",
                    ) {
                        state.error_message = Some(msg);
                    } else if let Err(msg) = validate_port_input(
                        &state.container_mcp_port_input,
                        "Container MCP port",
                    ) {
                        state.error_message = Some(msg);
                    } else {
                        state.step = SetupStep::Summary;
                    }
                }
                KeyCode::Tab | KeyCode::Down => {
                    state.next_ports_focus();
                }
                KeyCode::BackTab | KeyCode::Up => {
                    state.previous_ports_focus();
                }
                KeyCode::Char('t') => {
                    state.toggle_ports_boolean();
                }
                KeyCode::Backspace if state.ports_focus_is_text_field() => {
                    match state.ports_focus {
                        PortsField::GatewayPort => { state.gateway_port_input.pop(); }
                        PortsField::ContainerHostPort => { state.container_host_port_input.pop(); }
                        PortsField::ContainerMcpPort => { state.container_mcp_port_input.pop(); }
                        _ => {}
                    }
                }
                KeyCode::Char(ch) if state.ports_focus_is_text_field() && ch.is_ascii_digit() => {
                    match state.ports_focus {
                        PortsField::GatewayPort => state.gateway_port_input.push(ch),
                        PortsField::ContainerHostPort => state.container_host_port_input.push(ch),
                        PortsField::ContainerMcpPort => state.container_mcp_port_input.push(ch),
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
                    finalize_and_start(state, use_case).await;
                }
                KeyCode::Tab | KeyCode::Down => {
                    state.next_summary_focus();
                }
                KeyCode::BackTab | KeyCode::Up => {
                    state.previous_summary_focus();
                }
                KeyCode::Char(' ') | KeyCode::Char('t')
                    if !state.summary_focus_is_text_field() =>
                {
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
                _ => {}
            },
        }
    }
}

fn handle_runtime_tick(state: &mut FirstRunTuiState) {
    if matches!(state.step, SetupStep::RuntimeStatus) {
        state.refresh_runtime_logs();
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
            state.step = SetupStep::Auth;
        }
        Err(error) => {
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
            state.error_message = Some(error.to_string());
            state.info_message = None;
        }
    }
}

async fn finalize_and_start(state: &mut FirstRunTuiState, use_case: &FirstRunSetupUseCase) {
    state.clear_messages();
    state.info_message = Some("Writing config and starting Brain3...".into());

    let request: FinalizeSetupRequest = state.apply_inputs_to_draft();

    let summary = match use_case
        .finalize(request)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))
    {
        Ok(summary) => summary,
        Err(error) => {
            state.error_message = Some(error.to_string());
            state.info_message = None;
            return;
        }
    };

    let session = match start_configured_runtime_session(
        &state.host,
        RuntimeLaunchPlan {
            paths: summary.paths.clone(),
            env_file: summary.paths.env_file.clone(),
            log_file: state.log_file.clone(),
        },
    )
    .await
    {
        Ok(session) => session,
        Err(error) => {
            tracing::error!(error = %error, "failed to start gateway session");
            state.error_message = Some(error.to_string());
            state.info_message = None;
            return;
        }
    };

    state.summary = Some(summary);
    state.runtime = Some(session.runtime);
    state.server = session.server;

    if let Some(display_url) = session.display_url {
        let summary = state.summary.as_ref().expect("summary should be present");
        tracing::debug!(server_url = %display_url, "building connection card after first-run wizard");
        let connection_card =
            use_case.build_connection_card(display_url, state.log_file.clone(), summary);
        state.connection_card = Some(connection_card);
    }

    if let Some(runtime) = &state.runtime {
        if let Some(failure) = runtime.primary_failure_summary() {
            state.error_message = Some(failure.to_string());
            state.info_message = None;
            state.step = SetupStep::RuntimeStatus;
        } else {
            state.info_message = Some("Brain3 is running.".into());
            state.step = if state.connection_card.is_some() {
                SetupStep::ConnectionCard
            } else {
                SetupStep::RuntimeStatus
            };
        }
    }
}

async fn start_configured_runtime_session(
    host: &str,
    launch_plan: RuntimeLaunchPlan,
) -> Result<ConfiguredGatewaySession> {
    server::spawn_configured_gateway_session(host, launch_plan, RuntimeOverrides::default()).await
}

async fn cleanup(state: &mut FirstRunTuiState) -> Result<()> {
    if let Some(server) = state.server.take() {
        server.shutdown().await?;
    }
    state.runtime = None;
    Ok(())
}
