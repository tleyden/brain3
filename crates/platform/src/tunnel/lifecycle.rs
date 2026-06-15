use std::path::Path;

use tokio::process::Child;
use tokio::time::Duration;

pub async fn write_pid_file(pid_file: &Path, pid: u32) {
    match tokio::fs::write(pid_file, pid.to_string()).await {
        Ok(()) => tracing::info!(
            pid,
            pid_file = %pid_file.display(),
            "cloudflared PID file written"
        ),
        Err(e) => tracing::warn!(
            pid,
            pid_file = %pid_file.display(),
            error = %e,
            "failed to write cloudflared PID file"
        ),
    }
}

pub async fn remove_pid_file(pid_file: &Path) {
    match tokio::fs::remove_file(pid_file).await {
        Ok(()) => tracing::debug!(pid_file = %pid_file.display(), "cloudflared PID file removed"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(
            pid_file = %pid_file.display(),
            error = %e,
            "failed to remove cloudflared PID file"
        ),
    }
}

/// SIGTERM → poll at 50 ms / 100 ms / 250 ms → SIGKILL. Removes the PID file when done.
pub async fn graceful_kill(child: &mut Child, pid: u32, pid_file: &Path) {
    tracing::info!(
        pid,
        pid_file = %pid_file.display(),
        "initiating graceful shutdown of cloudflared"
    );

    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if ret != 0 {
            tracing::warn!(
                pid,
                errno = %std::io::Error::last_os_error(),
                "SIGTERM delivery to cloudflared failed"
            );
        } else {
            tracing::info!(pid, "SIGTERM sent to cloudflared");
        }
    }

    for (attempt, &delay_ms) in [50u64, 100, 250].iter().enumerate() {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        match child.try_wait() {
            Ok(Some(status)) => {
                tracing::info!(
                    pid,
                    exit_code = ?status.code(),
                    "cloudflared exited cleanly after SIGTERM"
                );
                remove_pid_file(pid_file).await;
                return;
            }
            Ok(None) => {
                tracing::warn!(
                    pid,
                    attempt = attempt + 1,
                    "cloudflared still running after SIGTERM, waiting longer"
                );
            }
            Err(e) => {
                tracing::warn!(pid, error = %e, "error polling cloudflared exit status");
            }
        }
    }

    tracing::warn!(
        pid,
        "cloudflared did not respond to SIGTERM within 400 ms — escalating to SIGKILL"
    );
    if let Err(e) = child.kill().await {
        tracing::warn!(pid, error = %e, "SIGKILL to cloudflared failed");
    } else {
        tracing::info!(pid, "cloudflared killed with SIGKILL");
    }
    remove_pid_file(pid_file).await;
}
