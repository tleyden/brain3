use std::path::PathBuf;
use std::sync::Arc;

use brain3_core::domain::errors::TunnelError;
use brain3_core::ports::tunnel::{TunnelInfo, TunnelPort, TunnelStatus};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub struct CloudflareNamedTunnelAdapter {
    tunnel_name: String,
    domain: String,
    config_file: PathBuf,
    child: Arc<Mutex<Option<Child>>>,
    public_url: Arc<Mutex<Option<String>>>,
}

impl CloudflareNamedTunnelAdapter {
    pub fn new(tunnel_name: impl Into<String>, domain: impl Into<String>, config_file: PathBuf) -> Self {
        Self {
            tunnel_name: tunnel_name.into(),
            domain: domain.into(),
            config_file,
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

        let mut child = cmd
            .spawn()
            .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

        let stderr = child.stderr.take().expect("stderr was piped");

        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("cloudflared: {}", line);
            }
        });

        let public_url = format!("https://{}.{}", self.tunnel_name, self.domain);
        *self.public_url.lock().await = Some(public_url.clone());
        *self.child.lock().await = Some(child);

        Ok(TunnelInfo { public_url })
    }

    async fn stop(&self) -> Result<(), TunnelError> {
        if let Some(mut child) = self.child.lock().await.take() {
            child.kill().await.map_err(|e| TunnelError::Other(e.to_string()))?;
        }
        *self.public_url.lock().await = None;
        Ok(())
    }

    async fn status(&self) -> Result<TunnelStatus, TunnelError> {
        let mut guard = self.child.lock().await;
        let Some(child) = guard.as_mut() else {
            return Ok(TunnelStatus::Stopped);
        };
        match child.try_wait().map_err(|e| TunnelError::Other(e.to_string()))? {
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
