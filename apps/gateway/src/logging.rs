use std::fs::OpenOptions;
use std::path::PathBuf;

use anyhow::{Context, Result};
use brain3_core::ports::setup_system::SetupSystemPort;
use brain3_platform::setup::PlatformSetupSystem;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

pub struct GatewayLogging {
    pub log_file: PathBuf,
    _guard: WorkerGuard,
}

pub async fn init_logging() -> Result<GatewayLogging> {
    let setup_system = PlatformSetupSystem::new();
    let log_file = setup_system
        .create_temp_log_file()
        .await
        .context("failed to allocate gateway log file")?;

    let file = OpenOptions::new()
        .append(true)
        .open(&log_file)
        .with_context(|| format!("failed to open gateway log file {}", log_file.display()))?;

    let (writer, guard) = tracing_appender::non_blocking(file);

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_writer(writer)
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing subscriber: {error}"))?;

    tracing::info!(log_file = %log_file.display(), "gateway logging initialized");

    Ok(GatewayLogging {
        log_file,
        _guard: guard,
    })
}
