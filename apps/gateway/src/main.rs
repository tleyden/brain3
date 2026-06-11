mod logging;
mod server;
mod setup_tui;
#[allow(dead_code)]
mod tui;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

use brain3_core::domain::model::TunnelConfig;
use brain3_core::domain::setup::RuntimeLaunchPlan;
use brain3_core::ports::config::ConfigPort;
use brain3_platform::config::env_file::EnvFileConfigAdapter;
use brain3_platform::runtime::bootstrap_configured_runtime;
use brain3_platform::setup::app_home::Brain3AppHome;

#[derive(Parser)]
#[command(name = "brain3", about = "OAuth2 gateway for MCP servers")]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long)]
    env_file: Option<PathBuf>,

    #[arg(
        long,
        help = "Run the interactive setup wizard for Cloudflare named tunnel provisioning"
    )]
    setup: bool,
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");
    tracing::info!("Received shutdown signal, draining connections...");
}

fn resolve_config_env_file(args: &Args) -> Result<(Brain3AppHome, PathBuf, bool)> {
    let app_home =
        Brain3AppHome::resolve_from_env().context("failed to resolve Brain3 app home")?;
    let using_default_env_file = args.env_file.is_none();
    let env_file = args
        .env_file
        .clone()
        .unwrap_or_else(|| app_home.env_file.clone());
    Ok((app_home, env_file, using_default_env_file))
}

fn exit_if_first_run(app_home: &Brain3AppHome, env_file: &Path) -> Result<()> {
    eprintln!(
        "\nBrain3 is not configured yet.\n\
         \n  App home: {}\n\
         \n  Expected config: {}\n\
         \nThe first-run setup wizard is not implemented in this slice yet.\n\
         Create the config file at that location before starting Brain3 again.\n",
        app_home.root_dir.display(),
        env_file.display()
    );
    tracing::warn!(
        app_home = %app_home.root_dir.display(),
        env_file = %env_file.display(),
        "first-run setup required"
    );
    anyhow::bail!("first-run setup required");
}

fn setup_requires_named_tunnel() -> Result<()> {
    eprintln!("--setup requires CF_TUNNEL_NAME and CF_DOMAIN to be set in your .env file.");
    anyhow::bail!("--setup requires CF_TUNNEL_NAME and CF_DOMAIN to be set");
}

#[tokio::main]
async fn main() -> Result<()> {
    let _logging = logging::init_logging().await?;

    let args = Args::parse();

    let (app_home, env_file, using_default_env_file) = resolve_config_env_file(&args)?;
    if using_default_env_file && !env_file.exists() {
        return exit_if_first_run(&app_home, &env_file);
    }

    let config_adapter = EnvFileConfigAdapter::new(Some(env_file.clone()));
    let config = Arc::new(
        config_adapter
            .load()
            .context("failed to load configuration")?,
    );

    if args.setup {
        match &config.tunnel {
            Some(tc @ TunnelConfig::CloudflareNamed { .. }) => {
                return setup_tui::run(tc).await;
            }
            _ => {
                return setup_requires_named_tunnel();
            }
        }
    }

    let runtime = bootstrap_configured_runtime(
        Arc::clone(&config),
        RuntimeLaunchPlan {
            paths: app_home.as_setup_paths(),
            env_file: env_file.clone(),
            log_file: _logging.log_file.clone(),
        },
    )
    .await?;

    let config = Arc::clone(&runtime.config);
    let upstream_secret = runtime.upstream_secret.clone();

    if let Some(public_url) = &runtime.public_url {
        tracing::info!(url = %public_url, "runtime public URL ready");
    }
    tracing::info!(
        container_status = ?runtime.container_status,
        tunnel_status = ?runtime.tunnel_status,
        log_file = %runtime.launch_plan.log_file.display(),
        "runtime bootstrap complete"
    );

    let _runtime = runtime;

    server::run_gateway_server_until(&args.host, config, upstream_secret, shutdown_signal())
        .await?;

    Ok(())
}
