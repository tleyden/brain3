use std::env;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use brain3_core::domain::setup::SetupPaths;
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

pub async fn init_logging(
    default_level: &str,
    brain3_home: Option<PathBuf>,
) -> Result<GatewayLogging> {
    let setup_system = match brain3_home {
        Some(dir) => PlatformSetupSystem::with_home_override(dir),
        None => PlatformSetupSystem::new(),
    };
    let paths = setup_system.resolve_paths().unwrap_or_else(|error| {
        let fallback_home = env::temp_dir().join("brain3");
        tracing::warn!(
            error = %error,
            fallback_home = %fallback_home.display(),
            "failed to resolve brain3 home for log file, falling back to temp dir"
        );
        SetupPaths::new(
            fallback_home.clone(),
            fallback_home.join(".env"),
            fallback_home.join("cloudflared"),
        )
    });
    let log_file = setup_system
        .resolve_log_file(&paths)
        .await
        .unwrap_or_else(|error| {
            let fallback_log_file = env::temp_dir().join("brain3.log");
            tracing::warn!(
                error = %error,
                fallback_log_file = %fallback_log_file.display(),
                "failed to resolve gateway log file, falling back to temp dir"
            );
            fallback_log_file
        });

    let mut log_options = OpenOptions::new();
    log_options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        log_options.mode(0o600);
    }

    let file = log_options
        .open(&log_file)
        .with_context(|| format!("failed to open gateway log file {}", log_file.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .with_context(|| {
                format!(
                    "failed to set gateway log file permissions on {}",
                    log_file.display()
                )
            })?;
    }

    let (writer, guard) = tracing_appender::non_blocking(file);
    let mirror_to_stderr = Arc::new(AtomicBool::new(false));

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level)),
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
