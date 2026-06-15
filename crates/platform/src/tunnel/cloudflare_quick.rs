use std::path::PathBuf;
use std::sync::Arc;

use brain3_core::domain::errors::TunnelError;
use brain3_core::ports::tunnel::{TunnelInfo, TunnelPort, TunnelStatus};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

use super::lifecycle;

pub struct CloudflareQuickTunnelAdapter {
    local_port: u16,
    pid_file: PathBuf,
    child: Arc<Mutex<Option<Child>>>,
    public_url: Arc<Mutex<Option<String>>>,
}

impl CloudflareQuickTunnelAdapter {
    pub fn new(local_port: u16, pid_file: PathBuf) -> Self {
        Self {
            local_port,
            pid_file,
            child: Arc::new(Mutex::new(None)),
            public_url: Arc::new(Mutex::new(None)),
        }
    }
}

fn cloudflared_on_path() -> bool {
    std::process::Command::new("which")
        .arg("cloudflared")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl TunnelPort for CloudflareQuickTunnelAdapter {
    async fn start(&self) -> Result<TunnelInfo, TunnelError> {
        if !cloudflared_on_path() {
            return Err(TunnelError::CloudflaredNotFound);
        }

        let mut cmd = Command::new("cloudflared");
        cmd.args([
            "tunnel",
            "--url",
            &format!("http://localhost:{}", self.local_port),
        ])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true);

        // On Linux: if our process dies unexpectedly cloudflared receives SIGTERM
        // (runs in the child after fork, before exec — cannot use tracing here).
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            tracing::debug!("configuring pdeathsig for cloudflared quick tunnel process");
            unsafe {
                cmd.as_std_mut().pre_exec(|| {
                    let _ = libc::prctl(
                        libc::PR_SET_PDEATHSIG,
                        libc::SIGTERM as libc::c_ulong,
                        0,
                        0,
                        0,
                    );
                    Ok(())
                });
            }
        }

        tracing::info!(
            local_port = self.local_port,
            pid_file = %self.pid_file.display(),
            "spawning cloudflared quick tunnel"
        );

        let mut child = cmd
            .spawn()
            .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

        let pid = child.id();
        tracing::info!(pid = ?pid, "cloudflared quick tunnel process spawned");

        if let Some(p) = pid {
            lifecycle::write_pid_file(&self.pid_file, p).await;
        }

        let stderr = child.stderr.take().expect("stderr was piped");

        let url_arc = Arc::clone(&self.public_url);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("cloudflared: {}", line);
                if url_arc.lock().await.is_none() {
                    if let Some(url) = extract_trycloudflare_url(&line) {
                        *url_arc.lock().await = Some(url);
                    }
                }
            }
        });

        let url = timeout(Duration::from_secs(30), async {
            loop {
                if let Some(u) = self.public_url.lock().await.clone() {
                    return u;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .map_err(|_| TunnelError::Other("timed out waiting for tunnel URL".into()))?;

        tracing::info!(url = %url, "cloudflared quick tunnel URL ready");

        *self.child.lock().await = Some(child);

        Ok(TunnelInfo { public_url: url })
    }

    async fn stop(&self) -> Result<(), TunnelError> {
        let Some(mut child) = self.child.lock().await.take() else {
            tracing::debug!("stop() called but no cloudflared quick tunnel process is running");
            lifecycle::remove_pid_file(&self.pid_file).await;
            return Ok(());
        };

        let pid = child.id();
        tracing::info!(pid = ?pid, "stopping cloudflared quick tunnel");

        if let Some(p) = pid {
            lifecycle::graceful_kill(&mut child, p, &self.pid_file).await;
        } else {
            tracing::info!("cloudflared quick tunnel process already exited");
            lifecycle::remove_pid_file(&self.pid_file).await;
        }

        *self.public_url.lock().await = None;
        tracing::info!("cloudflared quick tunnel stopped");
        Ok(())
    }

    async fn status(&self) -> Result<TunnelStatus, TunnelError> {
        let mut guard = self.child.lock().await;
        let Some(child) = guard.as_mut() else {
            return Ok(TunnelStatus::Stopped);
        };
        match child
            .try_wait()
            .map_err(|e| TunnelError::Other(e.to_string()))?
        {
            None => {
                let url = self.public_url.lock().await.clone().unwrap_or_default();
                Ok(TunnelStatus::Running(TunnelInfo { public_url: url }))
            }
            Some(_) => {
                *guard = None;
                Ok(TunnelStatus::Stopped)
            }
        }
    }
}

fn extract_trycloudflare_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|')
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.contains(".trycloudflare.com") {
        Some(url.to_string())
    } else {
        None
    }
}
