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
use brain3_core::domain::setup::{FinalizeSetupRequest, RuntimeLaunchPlan, SetupStep};
use brain3_core::ports::config::ConfigPort;
use brain3_core::ports::setup_system::SetupSystemPort;
use brain3_platform::config::env_file::EnvFileConfigAdapter;
use brain3_platform::runtime::{bootstrap_configured_runtime, RuntimeBootstrap};
use brain3_platform::setup::PlatformSetupSystem;

use crate::server;
use crate::server::GatewayServerHandle;

use super::screens;
use super::state::{AuthField, FirstRunTuiState};

pub enum GatewayTuiLaunch {
    FirstRun,
    Configured { launch_plan: RuntimeLaunchPlan },
}

struct StartedGatewaySession {
    runtime: RuntimeBootstrap,
    server_handle: GatewayServerHandle,
    server_url: String,
}

pub async fn run_gateway_tui(
    host: &str,
    log_file: std::path::PathBuf,
    launch: GatewayTuiLaunch,
) -> Result<()> {
    let setup_system: Arc<dyn SetupSystemPort> = Arc::new(PlatformSetupSystem::new());
    let use_case = FirstRunSetupUseCase::new(Arc::clone(&setup_system));
    let preparation = use_case
        .prepare()
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;

    let mut state = match launch {
        GatewayTuiLaunch::FirstRun => {
            FirstRunTuiState::new(host.to_string(), log_file.clone(), preparation)
        }
        GatewayTuiLaunch::Configured { launch_plan } => {
            let session = start_configured_runtime_session(host, launch_plan).await?;
            FirstRunTuiState::new_runtime(
                host.to_string(),
                log_file.clone(),
                preparation,
                session.runtime,
                session.server_handle,
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
                    state.step = SetupStep::VaultPath;
                }
                KeyCode::Char('r') => {
                    refresh_dependencies(state, Arc::clone(&setup_system)).await;
                }
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    if let Some(index) = ch.to_digit(10).map(|value| value as usize) {
                        run_install_action(state, Arc::clone(&setup_system), index).await;
                    }
                }
                _ => {}
            },
            SetupStep::VaultPath => match key.code {
                KeyCode::Esc => {
                    state.clear_messages();
                    state.step = SetupStep::DependencyDoctor;
                }
                KeyCode::Enter => {
                    state.clear_messages();
                    state.step = SetupStep::Auth;
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
                    state.step = SetupStep::Summary;
                }
                KeyCode::Tab => {
                    state.next_auth_focus();
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
            SetupStep::Summary => match key.code {
                KeyCode::Esc => {
                    state.clear_messages();
                    state.step = SetupStep::Auth;
                }
                KeyCode::Enter => {
                    finalize_and_start(state, use_case).await;
                }
                _ => {}
            },
            SetupStep::ConnectionCard => match key.code {
                KeyCode::Enter => {
                    state.clear_messages();
                    state.step = SetupStep::RuntimeStatus;
                }
                _ => {}
            },
            SetupStep::RuntimeStatus => {
                if key.code == KeyCode::Esc && state.connection_card.is_some() {
                    state.clear_messages();
                    state.step = SetupStep::ConnectionCard;
                }
            }
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
    index: usize,
) {
    state.clear_messages();
    let Some(action) = state
        .dependency_actions()
        .get(index.saturating_sub(1))
        .copied()
    else {
        state.error_message = Some("No install action mapped to that key.".into());
        return;
    };

    state.info_message = Some(format!("Running {:?}...", action));
    match setup_system.run_install_action(action).await {
        Ok(()) => {
            state.info_message = Some(format!(
                "{action:?} completed. Refreshing dependency status."
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
            state.error_message = Some(error.to_string());
            state.info_message = None;
            return;
        }
    };

    let connection_card =
        use_case.build_connection_card(session.server_url, state.log_file.clone(), &summary);

    state.summary = Some(summary);
    state.connection_card = Some(connection_card);
    state.runtime = Some(session.runtime);
    state.server = Some(session.server_handle);
    state.info_message = Some("Brain3 is running.".into());
    state.step = SetupStep::ConnectionCard;
}

async fn start_configured_runtime_session(
    host: &str,
    launch_plan: RuntimeLaunchPlan,
) -> Result<StartedGatewaySession> {
    let config = Arc::new(
        EnvFileConfigAdapter::new(Some(launch_plan.env_file.clone()))
            .load()
            .map_err(|error| anyhow::anyhow!("{error}"))?,
    );

    let runtime = bootstrap_configured_runtime(Arc::clone(&config), launch_plan).await?;
    let server_handle = server::spawn_gateway_server(
        host,
        Arc::clone(&runtime.config),
        runtime.upstream_secret.clone(),
    )
    .await?;
    let server_url = runtime
        .public_url
        .clone()
        .unwrap_or_else(|| server_handle.local_url().to_string());

    Ok(StartedGatewaySession {
        runtime,
        server_handle,
        server_url,
    })
}

async fn cleanup(state: &mut FirstRunTuiState) -> Result<()> {
    if let Some(server) = state.server.take() {
        server.shutdown().await?;
    }
    state.runtime = None;
    Ok(())
}
