use std::path::PathBuf;
use std::sync::Arc;

use brain3_core::domain::errors::TunnelError;
use brain3_core::ports::tunnel::{TunnelInfo, TunnelPort, TunnelStatus};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use super::lifecycle;

pub struct CloudflareNamedTunnelAdapter {
    tunnel_name: String,
    domain: String,
    config_file: PathBuf,
    pid_file: PathBuf,
    child: Arc<Mutex<Option<Child>>>,
    public_url: Arc<Mutex<Option<String>>>,
}

impl CloudflareNamedTunnelAdapter {
    pub fn new(
        tunnel_name: impl Into<String>,
        domain: impl Into<String>,
        config_file: PathBuf,
        pid_file: PathBuf,
    ) -> Self {
        Self {
            tunnel_name: tunnel_name.into(),
            domain: domain.into(),
            config_file,
            pid_file,
            child: Arc::new(Mutex::new(None)),
            public_url: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(serde::Deserialize)]
struct CfTunnel {
    name: String,
    connections: Vec<serde_json::Value>,
}

fn cloudflared_on_path() -> bool {
    std::process::Command::new("which")
        .arg("cloudflared")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns (registered, active_connection_count).
/// registered=false means the tunnel name does not exist in Cloudflare's registry.
async fn check_cf_registry(tunnel_name: &str) -> (bool, usize) {
    let output = match Command::new("cloudflared")
        .args(["tunnel", "list", "--output", "json", "--name", tunnel_name])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = %e, "could not run `cloudflared tunnel list` for registry check");
            return (false, 0);
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let tunnels: Vec<CfTunnel> = match serde_json::from_str(&stdout) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, raw = %stdout, "could not parse `cloudflared tunnel list` output");
            return (false, 0);
        }
    };

    for t in &tunnels {
        if t.name == tunnel_name {
            return (true, t.connections.len());
        }
    }

    (false, 0)
}

async fn cleanup_tunnel(tunnel_name: &str) -> Result<(), TunnelError> {
    let output = Command::new("cloudflared")
        .args(["tunnel", "cleanup", tunnel_name])
        .output()
        .await
        .map_err(|e| TunnelError::Other(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TunnelError::Other(format!(
            "cloudflared tunnel cleanup failed: {stderr}"
        )));
    }

    Ok(())
}

#[async_trait::async_trait]
impl TunnelPort for CloudflareNamedTunnelAdapter {
    async fn start(&self) -> Result<TunnelInfo, TunnelError> {
        if !cloudflared_on_path() {
            return Err(TunnelError::CloudflaredNotFound);
        }

        if !self.config_file.exists() {
            return Err(TunnelError::ConfigNotFound(
                self.config_file.display().to_string(),
            ));
        }
        tracing::info!(
            config_file = %self.config_file.display(),
            "cloudflare tunnel config file present"
        );

        // Pre-start: cleanup any stale connections before spawning.
        let (registered_before, stale_connections) = check_cf_registry(&self.tunnel_name).await;
        tracing::info!(
            tunnel_name = %self.tunnel_name,
            registered = registered_before,
            stale_connections,
            "pre-start CF registry check"
        );
        if registered_before && stale_connections > 0 {
            tracing::warn!(
                tunnel = %self.tunnel_name,
                connections = stale_connections,
                "found stale tunnel connections — running cleanup before start"
            );
            cleanup_tunnel(&self.tunnel_name).await?;
        }

        let mut cmd = Command::new("cloudflared");
        cmd.args([
            "tunnel",
            "--config",
            &self.config_file.display().to_string(),
            "run",
            &self.tunnel_name,
        ])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true);

        // On Linux: if our process dies unexpectedly cloudflared receives SIGTERM
        // (runs in the child after fork, before exec — cannot use tracing here).
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            tracing::debug!(
                tunnel_name = %self.tunnel_name,
                "configuring pdeathsig for cloudflared named tunnel process"
            );
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
            tunnel_name = %self.tunnel_name,
            pid_file = %self.pid_file.display(),
            "spawning cloudflared named tunnel"
        );

        let mut child = cmd
            .spawn()
            .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

        let pid = child.id();
        tracing::info!(tunnel_name = %self.tunnel_name, pid = ?pid, "cloudflared named tunnel process spawned");

        if let Some(p) = pid {
            lifecycle::write_pid_file(&self.pid_file, p).await;
        }

        let stderr = child.stderr.take().expect("stderr was piped");

        // Store child and URL immediately so the stderr logger can start and
        // so stop() works even if diagnostics fail below.
        let public_url = format!("https://{}.{}", self.tunnel_name, self.domain);
        *self.public_url.lock().await = Some(public_url.clone());
        *self.child.lock().await = Some(child);

        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!("cloudflared: {}", line);
            }
        });

        // Wait 2s to let cloudflared either connect or fail fast.
        sleep(Duration::from_secs(2)).await;

        // --- Check 1: process still alive ---
        let process_alive = {
            let mut guard = self.child.lock().await;
            match guard.as_mut() {
                None => false,
                Some(child) => child.try_wait().map(|r| r.is_none()).unwrap_or(false),
            }
        };
        tracing::info!(
            tunnel_name = %self.tunnel_name,
            process_alive,
            "tunnel process check"
        );
        if !process_alive {
            return Err(TunnelError::Other(format!(
                "cloudflared process for tunnel '{}' exited immediately after launch — \
                 check logs above for the error from cloudflared",
                self.tunnel_name
            )));
        }

        // --- Check 2: Cloudflare registry ---
        let (registered, active_connections) = check_cf_registry(&self.tunnel_name).await;
        tracing::info!(
            tunnel_name = %self.tunnel_name,
            registered,
            active_connections,
            "post-start CF registry check"
        );
        if !registered {
            return Err(TunnelError::TunnelNotFound(self.tunnel_name.clone()));
        }

        tracing::info!(
            tunnel_name = %self.tunnel_name,
            url = %public_url,
            "cloudflared named tunnel ready"
        );

        Ok(TunnelInfo { public_url })
    }

    async fn stop(&self) -> Result<(), TunnelError> {
        let Some(mut child) = self.child.lock().await.take() else {
            tracing::debug!(
                tunnel_name = %self.tunnel_name,
                "stop() called but no cloudflared named tunnel process is running"
            );
            lifecycle::remove_pid_file(&self.pid_file).await;
            return Ok(());
        };

        let pid = child.id();
        tracing::info!(tunnel_name = %self.tunnel_name, pid = ?pid, "stopping cloudflared named tunnel");

        if let Some(p) = pid {
            lifecycle::graceful_kill(&mut child, p, &self.pid_file).await;
        } else {
            tracing::info!(tunnel_name = %self.tunnel_name, "cloudflared named tunnel process already exited");
            lifecycle::remove_pid_file(&self.pid_file).await;
        }

        *self.public_url.lock().await = None;

        match cleanup_tunnel(&self.tunnel_name).await {
            Ok(()) => tracing::info!(tunnel = %self.tunnel_name, "tunnel connections cleaned up"),
            Err(e) => {
                tracing::warn!(tunnel = %self.tunnel_name, error = %e, "tunnel cleanup after stop failed")
            }
        }

        tracing::info!(tunnel_name = %self.tunnel_name, "cloudflared named tunnel stopped");
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
