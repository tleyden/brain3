use std::path::PathBuf;
use std::time::Duration;

use brain3_core::domain::model::TunnelConfig;
use brain3_platform::tunnel::cloudflare_setup as cf;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Terminal;
use tokio::io::AsyncBufReadExt;

#[derive(Clone, PartialEq)]
enum StepStatus {
    Pending,
    Running,
    Done,
    Failed,
}

struct Step {
    label: String,
    status: StepStatus,
}

impl Step {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            status: StepStatus::Pending,
        }
    }

    fn icon(&self) -> Span<'_> {
        match self.status {
            StepStatus::Pending => Span::styled("·", Style::default().fg(Color::DarkGray)),
            StepStatus::Running => Span::styled("→", Style::default().fg(Color::Yellow)),
            StepStatus::Done => Span::styled("✓", Style::default().fg(Color::Green)),
            StepStatus::Failed => Span::styled("✗", Style::default().fg(Color::Red)),
        }
    }
}

pub struct SetupState {
    tunnel_name: String,
    domain: String,
    config_file: PathBuf,
    local_port: u16,
    steps: Vec<Step>,
    log: Vec<String>,
    running: bool,
    done: bool,
    // true = user is confirming, cursor on Run button
    focused_run: bool,
}

impl SetupState {
    fn new(tunnel_name: &str, domain: &str, config_file: PathBuf, local_port: u16) -> Self {
        let steps = vec![
            Step::new("cloudflared installed"),
            Step::new("cloudflared logged in"),
            Step::new(format!("tunnel \"{tunnel_name}\" exists").as_str()),
            Step::new("credentials file"),
            Step::new("config file written"),
            Step::new("DNS route"),
        ];
        Self {
            tunnel_name: tunnel_name.to_string(),
            domain: domain.to_string(),
            config_file,
            local_port,
            steps,
            log: Vec::new(),
            running: false,
            done: false,
            focused_run: true,
        }
    }

    fn log(&mut self, msg: impl Into<String>) {
        self.log.push(msg.into());
    }
}

pub async fn run(tunnel_config: &TunnelConfig) -> anyhow::Result<()> {
    let (tunnel_name, domain, config_file, local_port) = match tunnel_config {
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
        _ => {
            eprintln!("Setup wizard only available for named Cloudflare tunnel mode.");
            eprintln!("Set CF_TUNNEL_NAME and CF_DOMAIN in your .env file first.");
            return Ok(());
        }
    };

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = SetupState::new(&tunnel_name, &domain, config_file, local_port);

    // Initial read-only checks
    run_initial_checks(&mut state).await;

    let result = event_loop(&mut terminal, &mut state).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_initial_checks(state: &mut SetupState) {
    state.log("Running initial checks…");

    state.steps[0].status = StepStatus::Running;
    let installed = cf::check_cloudflared_installed().await;
    state.steps[0].status = if installed {
        StepStatus::Done
    } else {
        StepStatus::Failed
    };
    state.log(if installed {
        "cloudflared: found".to_string()
    } else {
        "cloudflared: not found — install from https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/".to_string()
    });

    if !installed {
        return;
    }

    state.steps[1].status = StepStatus::Running;
    let logged_in = cf::check_cloudflared_logged_in().await;
    state.steps[1].status = if logged_in {
        StepStatus::Done
    } else {
        StepStatus::Failed
    };
    state.log(if logged_in {
        "cloudflared login: ok".to_string()
    } else {
        "cloudflared login: not logged in".to_string()
    });

    if !logged_in {
        return;
    }

    state.steps[2].status = StepStatus::Running;
    match cf::find_tunnel_id(&state.tunnel_name).await {
        Ok(Some(id)) => {
            state.steps[2].status = StepStatus::Done;
            state.log(format!(
                "tunnel \"{}\": found ({})",
                state.tunnel_name,
                &id[..8]
            ));

            state.steps[3].status = StepStatus::Running;
            match cf::find_credentials_file(&id) {
                Some(p) => {
                    state.steps[3].status = StepStatus::Done;
                    state.log(format!("credentials: {}", p.display()));
                }
                None => {
                    state.steps[3].status = StepStatus::Failed;
                    state.log(format!("credentials: not found for tunnel {id}"));
                }
            }
        }
        Ok(None) => {
            state.steps[2].status = StepStatus::Failed;
            state.log(format!(
                "tunnel \"{}\": not found — will create",
                state.tunnel_name
            ));
        }
        Err(e) => {
            state.steps[2].status = StepStatus::Failed;
            state.log(format!("tunnel check failed: {e}"));
        }
    }

    if state.config_file.exists() {
        state.steps[4].status = StepStatus::Done;
        state.log(format!("config file: {}", state.config_file.display()));
    } else {
        state.steps[4].status = StepStatus::Failed;
        state.log(format!(
            "config file: not found at {}",
            state.config_file.display()
        ));
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut SetupState,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| draw(f, state))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Enter => {
                        if state.focused_run && !state.running && !state.done {
                            state.running = true;
                            run_setup(state).await;
                            state.running = false;
                            state.done = true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn draw(f: &mut ratatui::Frame, state: &SetupState) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Min(10),    // checklist + button
            Constraint::Length(10), // log panel
        ])
        .split(area);

    // Header
    let hostname = format!("{}.{}", state.tunnel_name, state.domain);
    let header = Paragraph::new(format!("Cloudflare Named Tunnel  ·  {hostname}"))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" brain3 Setup "),
        )
        .style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(header, chunks[0]);

    // Checklist
    let items: Vec<ListItem> = state
        .steps
        .iter()
        .map(|s| {
            ListItem::new(Line::from(vec![
                s.icon(),
                Span::raw("  "),
                Span::raw(s.label.clone()),
            ]))
        })
        .collect();

    let run_style = if state.focused_run && !state.done {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if state.done {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let button_line = if state.done {
        Line::from(Span::styled(
            "Setup complete!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))
    } else if state.running {
        Line::from(Span::styled(
            " Running… ",
            Style::default().fg(Color::Yellow),
        ))
    } else {
        Line::from(Span::styled(" [ Run Setup ] ", run_style))
    };

    let hint = Line::from(vec![
        Span::styled("[Enter]", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" run    "),
        Span::styled("[q]", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" quit"),
    ]);

    let mid_block = Block::default().borders(Borders::ALL).title(" Steps ");
    let inner = mid_block.inner(chunks[1]);
    f.render_widget(mid_block, chunks[1]);

    let mid_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(List::new(items), mid_chunks[0]);
    f.render_widget(Paragraph::new(button_line), mid_chunks[1]);
    f.render_widget(Paragraph::new(hint), mid_chunks[2]);

    // Log panel
    let log_text: Vec<Line> = state
        .log
        .iter()
        .rev()
        .take(8)
        .rev()
        .map(|l| Line::from(l.as_str()))
        .collect();
    let log = Paragraph::new(log_text)
        .block(Block::default().borders(Borders::ALL).title(" Log "))
        .wrap(Wrap { trim: false });
    f.render_widget(log, chunks[2]);
}

async fn run_setup(state: &mut SetupState) {
    state.log("Starting setup…");

    // Step 0: cloudflared installed (already checked)
    if state.steps[0].status != StepStatus::Done {
        state.log("Cannot proceed: cloudflared is not installed.");
        return;
    }

    // Step 1: login if needed
    if state.steps[1].status != StepStatus::Done {
        state.steps[1].status = StepStatus::Running;
        state.log("Launching `cloudflared tunnel login` — a browser window may open.");
        state.log("If no browser opens, copy the URL shown below into another machine's browser.");

        match cf::spawn_cloudflared_login().await {
            Ok(mut child) => {
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();

                // Drain stdout
                if let Some(s) = stdout {
                    let mut lines = tokio::io::BufReader::new(s).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        state.log(format!("  {line}"));
                    }
                }
                // Drain stderr (login URL appears here)
                if let Some(s) = stderr {
                    let mut lines = tokio::io::BufReader::new(s).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        state.log(format!("  {line}"));
                    }
                }

                match child.wait().await {
                    Ok(status) if status.success() => {
                        state.steps[1].status = StepStatus::Done;
                        state.log("Login successful.");
                    }
                    Ok(status) => {
                        state.steps[1].status = StepStatus::Failed;
                        state.log(format!("Login failed (exit {status})."));
                        return;
                    }
                    Err(e) => {
                        state.steps[1].status = StepStatus::Failed;
                        state.log(format!("Login error: {e}"));
                        return;
                    }
                }
            }
            Err(e) => {
                state.steps[1].status = StepStatus::Failed;
                state.log(format!("Could not spawn cloudflared: {e}"));
                return;
            }
        }
    }

    // Step 2: find or create tunnel
    state.steps[2].status = StepStatus::Running;
    state.log(format!("Looking up tunnel \"{}\"…", state.tunnel_name));
    let tunnel_id = match cf::find_tunnel_id(&state.tunnel_name).await {
        Ok(Some(id)) => {
            state.steps[2].status = StepStatus::Done;
            state.log(format!("Tunnel found: {id}"));
            id
        }
        Ok(None) => {
            state.log(format!("Creating tunnel \"{}\"…", state.tunnel_name));
            match cf::create_tunnel(&state.tunnel_name).await {
                Ok(id) => {
                    state.steps[2].status = StepStatus::Done;
                    state.log(format!("Tunnel created: {id}"));
                    id
                }
                Err(e) => {
                    state.steps[2].status = StepStatus::Failed;
                    state.log(format!("Failed to create tunnel: {e}"));
                    return;
                }
            }
        }
        Err(e) => {
            state.steps[2].status = StepStatus::Failed;
            state.log(format!("Tunnel lookup failed: {e}"));
            return;
        }
    };

    // Step 3: credentials file
    state.steps[3].status = StepStatus::Running;
    let creds_file = match cf::find_credentials_file(&tunnel_id) {
        Some(p) => {
            state.steps[3].status = StepStatus::Done;
            state.log(format!("Credentials: {}", p.display()));
            p
        }
        None => {
            state.steps[3].status = StepStatus::Failed;
            state.log(format!(
                "Credentials file not found for tunnel {tunnel_id}. \
                 Delete the tunnel and re-run setup: `cloudflared tunnel delete {}`",
                state.tunnel_name
            ));
            return;
        }
    };

    // Step 4: write config file
    state.steps[4].status = StepStatus::Running;
    state.log(format!(
        "Writing config to {}…",
        state.config_file.display()
    ));
    match cf::write_config_file(
        &state.config_file,
        &tunnel_id,
        &creds_file,
        &state.tunnel_name,
        &state.domain,
        state.local_port,
    ) {
        Ok(()) => {
            state.steps[4].status = StepStatus::Done;
            state.log("Config file written.");
        }
        Err(e) => {
            state.steps[4].status = StepStatus::Failed;
            state.log(format!("Failed to write config: {e}"));
            return;
        }
    }

    // Step 5: DNS route
    state.steps[5].status = StepStatus::Running;
    let hostname = format!("{}.{}", state.tunnel_name, state.domain);
    state.log(format!("Setting up DNS route for {hostname}…"));
    match cf::ensure_dns_route(&state.tunnel_name, &hostname).await {
        Ok(()) => {
            state.steps[5].status = StepStatus::Done;
            state.log(format!("DNS route: {hostname} → tunnel"));
        }
        Err(e) => {
            state.steps[5].status = StepStatus::Failed;
            state.log(format!("DNS route failed: {e}"));
            return;
        }
    }

    state.log(format!("Setup complete. Start the gateway with:  brain3"));
}
