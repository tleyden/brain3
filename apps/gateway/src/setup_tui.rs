use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use brain3_core::domain::model::TunnelConfig;
use brain3_platform::tunnel::cloudflare_setup;

#[derive(Clone, PartialEq)]
enum StepStatus {
    Checking,
    Done,
    Failed(String),
    NotStarted,
}

impl StepStatus {
    fn symbol(&self) -> &str {
        match self {
            StepStatus::Done => "✓",
            StepStatus::Failed(_) => "✗",
            StepStatus::Checking => "…",
            StepStatus::NotStarted => "·",
        }
    }

    fn style(&self) -> Style {
        match self {
            StepStatus::Done => Style::default().fg(Color::Green),
            StepStatus::Failed(_) => Style::default().fg(Color::Red),
            StepStatus::Checking => Style::default().fg(Color::Yellow),
            _ => Style::default().fg(Color::DarkGray),
        }
    }
}

struct SetupState {
    tunnel_name: String,
    domain: String,
    hostname: String,
    config_file: PathBuf,
    local_port: u16,
    steps: Vec<(&'static str, StepStatus)>,
    log_lines: Arc<Mutex<Vec<String>>>,
    running: bool,
    finished: bool,
    selected: usize,
}

impl SetupState {
    fn new(tunnel_name: String, domain: String, config_file: PathBuf, local_port: u16) -> Self {
        let hostname = format!("{tunnel_name}.{domain}");
        Self {
            tunnel_name,
            domain,
            hostname,
            config_file,
            local_port,
            steps: vec![
                ("cloudflared installed", StepStatus::NotStarted),
                ("cloudflared logged in", StepStatus::NotStarted),
                ("tunnel exists", StepStatus::NotStarted),
                ("credentials file", StepStatus::NotStarted),
                ("config file written", StepStatus::NotStarted),
                ("DNS route", StepStatus::NotStarted),
            ],
            log_lines: Arc::new(Mutex::new(Vec::new())),
            running: false,
            finished: false,
            selected: 0,
        }
    }

    fn log(&self, msg: impl Into<String>) {
        self.log_lines.lock().unwrap().push(msg.into());
    }

    fn all_done(&self) -> bool {
        self.steps.iter().all(|(_, s)| *s == StepStatus::Done)
    }
}

pub async fn run(config: &TunnelConfig) -> Result<()> {
    let (tunnel_name, domain, config_file, local_port) = match config {
        TunnelConfig::CloudflareNamed {
            tunnel_name,
            domain,
            config_file,
            local_port,
        } => (
            tunnel_name.clone(),
            domain.clone(),
            config_file.clone(),
            *local_port,
        ),
        _ => anyhow::bail!("--setup is only needed for named Cloudflare tunnels"),
    };

    let mut state = SetupState::new(tunnel_name, domain, config_file, local_port);

    enable_raw_mode()?;
    crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    // Run initial status check
    check_current_status(&mut state).await;

    let result = run_event_loop(&mut terminal, &mut state).await;

    disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;

    if state.all_done() {
        println!();
        println!("Setup complete!");
        println!("  Config file: {}", state.config_file.display());
        println!("  Public hostname: https://{}", state.hostname);
        println!();
        println!("Start the gateway:");
        println!("  cargo run --release -p brain3-gateway");
        println!();
    }

    result
}

async fn check_current_status(state: &mut SetupState) {
    state.log("Checking current status...");

    // Step 0: cloudflared installed
    state.steps[0].1 = StepStatus::Checking;
    let installed = cloudflare_setup::is_cloudflared_installed();
    state.steps[0].1 = if installed {
        state.log("cloudflared is installed");
        StepStatus::Done
    } else {
        state.log("cloudflared is NOT installed");
        StepStatus::Failed("not found in PATH".into())
    };
    if !installed {
        return;
    }

    // Step 1: logged in
    state.steps[1].1 = StepStatus::Checking;
    let logged_in = cloudflare_setup::is_cloudflared_logged_in().await;
    state.steps[1].1 = if logged_in {
        state.log("cloudflared is logged in");
        StepStatus::Done
    } else {
        state.log("cloudflared is NOT logged in");
        StepStatus::Failed("not logged in".into())
    };
    if !logged_in {
        return;
    }

    // Step 2: tunnel exists
    state.steps[2].1 = StepStatus::Checking;
    match cloudflare_setup::find_tunnel_id(&state.tunnel_name).await {
        Ok(Some(id)) => {
            state.log(format!("tunnel '{}' exists (ID: {})", state.tunnel_name, id));
            state.steps[2].1 = StepStatus::Done;

            // Step 3: credentials file
            state.steps[3].1 = StepStatus::Checking;
            match cloudflare_setup::find_credentials_file(&id) {
                Ok(path) => {
                    state.log(format!("credentials file found: {}", path.display()));
                    state.steps[3].1 = StepStatus::Done;
                }
                Err(_) => {
                    state.log("credentials file NOT found");
                    state.steps[3].1 = StepStatus::Failed("missing".into());
                }
            }
        }
        Ok(None) => {
            state.log(format!("tunnel '{}' does not exist", state.tunnel_name));
            state.steps[2].1 = StepStatus::Failed("not created".into());
        }
        Err(e) => {
            state.log(format!("error checking tunnel: {e}"));
            state.steps[2].1 = StepStatus::Failed(e.to_string());
        }
    }

    // Step 4: config file
    state.steps[4].1 = if state.config_file.exists() {
        state.log(format!(
            "config file exists: {}",
            state.config_file.display()
        ));
        StepStatus::Done
    } else {
        state.log(format!(
            "config file NOT found: {}",
            state.config_file.display()
        ));
        StepStatus::Failed("missing".into())
    };
}

async fn run_setup(state: &mut SetupState) {
    state.running = true;
    state.log("");
    state.log("=== Starting setup ===");

    // Step 0: cloudflared installed
    if state.steps[0].1 != StepStatus::Done {
        state.steps[0].1 = StepStatus::Checking;
        if cloudflare_setup::is_cloudflared_installed() {
            state.steps[0].1 = StepStatus::Done;
            state.log("cloudflared found");
        } else {
            state.steps[0].1 = StepStatus::Failed("not installed".into());
            state.log("ERROR: cloudflared is not installed");
            state.log("Install with: brew install cloudflare/cloudflare/cloudflared");
            state.log("Or see: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/");
            state.running = false;
            return;
        }
    }

    // Step 1: cloudflared login
    if state.steps[1].1 != StepStatus::Done {
        state.steps[1].1 = StepStatus::Checking;
        if cloudflare_setup::is_cloudflared_logged_in().await {
            state.steps[1].1 = StepStatus::Done;
            state.log("Already logged in");
        } else {
            state.log("");
            state.log("Starting cloudflared login...");
            state.log("This will try to open a browser. If it doesn't open,");
            state.log("copy the URL shown below and open it manually.");
            let domain = state.domain.clone();
            state.log(format!(
                "When prompted, authorize the domain: {domain}"
            ));
            state.log("");

            let log_lines = Arc::clone(&state.log_lines);
            let result = cloudflare_setup::run_cloudflared_login(|line| {
                log_lines.lock().unwrap().push(format!("  cloudflared: {line}"));
            })
            .await;

            match result {
                Ok(()) => {
                    state.steps[1].1 = StepStatus::Done;
                    state.log("Login successful!");
                }
                Err(e) => {
                    state.steps[1].1 = StepStatus::Failed("login failed".into());
                    state.log(format!("Login failed: {e}"));
                    state.running = false;
                    return;
                }
            }
        }
    }

    // Step 2: ensure tunnel
    state.steps[2].1 = StepStatus::Checking;
    let tunnel_name = state.tunnel_name.clone();
    state.log(format!("Ensuring tunnel '{tunnel_name}' exists..."));
    let tunnel_id = match cloudflare_setup::ensure_tunnel(&tunnel_name).await {
        Ok(id) => {
            state.steps[2].1 = StepStatus::Done;
            state.log(format!("Tunnel ready (ID: {id})"));
            id
        }
        Err(e) => {
            state.steps[2].1 = StepStatus::Failed("failed".into());
            state.log(format!("Failed to create tunnel: {e}"));
            state.running = false;
            return;
        }
    };

    // Step 3: credentials file
    state.steps[3].1 = StepStatus::Checking;
    let credentials_file = match cloudflare_setup::find_credentials_file(&tunnel_id) {
        Ok(path) => {
            state.steps[3].1 = StepStatus::Done;
            state.log(format!("Credentials file: {}", path.display()));
            path
        }
        Err(e) => {
            state.steps[3].1 = StepStatus::Failed("missing".into());
            state.log(format!("Credentials file not found: {e}"));
            state.log("If this tunnel was created on another machine, copy the credentials file to this machine.");
            state.running = false;
            return;
        }
    };

    // Step 4: write config
    state.steps[4].1 = StepStatus::Checking;
    let config_path = state.config_file.clone();
    let hostname = state.hostname.clone();
    let local_port = state.local_port;
    state.log(format!("Writing config to {}...", config_path.display()));
    match cloudflare_setup::write_config_file(
        &tunnel_id,
        &credentials_file,
        &hostname,
        local_port,
        &config_path,
    ) {
        Ok(()) => {
            state.steps[4].1 = StepStatus::Done;
            state.log("Config file written");
        }
        Err(e) => {
            state.steps[4].1 = StepStatus::Failed("write failed".into());
            state.log(format!("Failed to write config: {e}"));
            state.running = false;
            return;
        }
    }

    // Step 5: DNS route
    state.steps[5].1 = StepStatus::Checking;
    state.log(format!("Setting DNS route for {hostname}..."));
    match cloudflare_setup::ensure_dns_route(&tunnel_name, &hostname).await {
        Ok(()) => {
            state.steps[5].1 = StepStatus::Done;
            state.log("DNS route configured");
        }
        Err(e) => {
            state.steps[5].1 = StepStatus::Failed("failed".into());
            state.log(format!("Failed to set DNS route: {e}"));
            state.running = false;
            return;
        }
    }

    state.log("");
    state.log("=== All steps complete! Press q to exit. ===");
    state.running = false;
    state.finished = true;
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut SetupState,
) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, state))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Enter if !state.running => {
                        if state.selected == 0 && !state.finished {
                            run_setup(state).await;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn draw(f: &mut Frame, state: &SetupState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(state.steps.len() as u16 + 2),
            Constraint::Length(3),
            Constraint::Min(6),
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("brain3 Setup Wizard")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(title, chunks[0]);

    // Tunnel info
    let info = Paragraph::new(format!(
        "Cloudflare Named Tunnel: {} → https://{}",
        state.tunnel_name, state.hostname
    ))
    .style(Style::default().fg(Color::White));
    f.render_widget(info, chunks[1]);

    // Steps checklist
    let items: Vec<ListItem> = state
        .steps
        .iter()
        .map(|(label, status)| {
            let line = Line::from(vec![
                Span::styled(
                    format!("  {} ", status.symbol()),
                    status.style(),
                ),
                Span::raw(*label),
                match status {
                    StepStatus::Failed(msg) => {
                        Span::styled(format!(" ({msg})"), Style::default().fg(Color::Red))
                    }
                    _ => Span::raw(""),
                },
            ]);
            ListItem::new(line)
        })
        .collect();

    let steps_list = List::new(items)
        .block(Block::default().title("Setup Steps").borders(Borders::ALL));
    f.render_widget(steps_list, chunks[2]);

    // Action bar
    let action_text = if state.running {
        "Running setup...".to_string()
    } else if state.finished || state.all_done() {
        "All done! Press [q] to exit.".to_string()
    } else {
        "Press [Enter] to run setup  |  [q] Quit".to_string()
    };

    let action_style = if state.running {
        Style::default().fg(Color::Yellow)
    } else if state.finished || state.all_done() {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Cyan)
    };

    let action = Paragraph::new(action_text).style(action_style);
    f.render_widget(action, chunks[3]);

    // Log panel
    let log_lines = state.log_lines.lock().unwrap();
    let visible_height = chunks[4].height.saturating_sub(2) as usize;
    let start = log_lines.len().saturating_sub(visible_height);
    let visible: Vec<Line> = log_lines[start..]
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect();
    let log_panel = Paragraph::new(visible)
        .block(Block::default().title("Log").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    f.render_widget(log_panel, chunks[4]);
}
