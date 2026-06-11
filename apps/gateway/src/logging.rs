use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use brain3_core::ports::setup_system::SetupSystemPort;
use brain3_platform::setup::PlatformSetupSystem;
use tracing_appender::non_blocking::NonBlocking;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::EnvFilter;

pub struct GatewayLogging {
    pub log_file: PathBuf,
    mirror_to_stderr: Arc<AtomicBool>,
    _guard: WorkerGuard,
}

impl GatewayLogging {
    pub fn enable_terminal_mirror(&self) {
        self.mirror_to_stderr.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct GatewayMakeWriter {
    file: NonBlocking,
    mirror_to_stderr: Arc<AtomicBool>,
}

impl<'a> MakeWriter<'a> for GatewayMakeWriter {
    type Writer = GatewayWriter;

    fn make_writer(&'a self) -> Self::Writer {
        GatewayWriter {
            file: self.file.clone(),
            stderr: self
                .mirror_to_stderr
                .load(Ordering::Relaxed)
                .then(io::stderr),
        }
    }
}

struct GatewayWriter {
    file: NonBlocking,
    stderr: Option<io::Stderr>,
}

impl Write for GatewayWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.file.write(buf)?;
        if let Some(stderr) = &mut self.stderr {
            stderr.write_all(&buf[..written])?;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()?;
        if let Some(stderr) = &mut self.stderr {
            stderr.flush()?;
        }
        Ok(())
    }
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
    let mirror_to_stderr = Arc::new(AtomicBool::new(false));

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_writer(GatewayMakeWriter {
            file: writer,
            mirror_to_stderr: Arc::clone(&mirror_to_stderr),
        })
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing subscriber: {error}"))?;

    tracing::info!(log_file = %log_file.display(), "gateway logging initialized");

    Ok(GatewayLogging {
        log_file,
        mirror_to_stderr,
        _guard: guard,
    })
}
