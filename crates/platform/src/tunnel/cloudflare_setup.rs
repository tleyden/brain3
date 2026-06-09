use std::path::PathBuf;

use brain3_core::domain::errors::TunnelError;
use tokio::process::Command;

pub fn is_cloudflared_installed() -> bool {
    std::process::Command::new("which")
        .arg("cloudflared")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub async fn is_cloudflared_logged_in() -> bool {
    Command::new("cloudflared")
        .args(["tunnel", "list"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

pub async fn run_cloudflared_login(
    mut on_output: impl FnMut(&str),
) -> Result<(), TunnelError> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut child = Command::new("cloudflared")
        .args(["tunnel", "login"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

    let stderr = child.stderr.take().expect("stderr was piped");
    let stdout = child.stdout.take().expect("stdout was piped");

    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stdout_lines = BufReader::new(stdout).lines();

    loop {
        tokio::select! {
            line = stderr_lines.next_line() => {
                match line {
                    Ok(Some(l)) => on_output(&l),
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            line = stdout_lines.next_line() => {
                match line {
                    Ok(Some(l)) => on_output(&l),
                    Ok(None) => {}
                    Err(_) => {}
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| TunnelError::SetupFailed(e.to_string()))?;

    if status.success() {
        Ok(())
    } else {
        Err(TunnelError::SetupFailed(
            "cloudflared tunnel login failed".into(),
        ))
    }
}

pub async fn find_tunnel_id(tunnel_name: &str) -> Result<Option<String>, TunnelError> {
    let output = Command::new("cloudflared")
        .args(["tunnel", "list", "-n", tunnel_name])
        .output()
        .await
        .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 2 && cols[1] == tunnel_name {
            return Ok(Some(cols[0].to_string()));
        }
    }
    Ok(None)
}

pub async fn create_tunnel(tunnel_name: &str) -> Result<String, TunnelError> {
    let output = Command::new("cloudflared")
        .args(["tunnel", "create", tunnel_name])
        .output()
        .await
        .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TunnelError::SetupFailed(format!(
            "cloudflared tunnel create failed: {stderr}"
        )));
    }

    find_tunnel_id(tunnel_name).await?.ok_or_else(|| {
        TunnelError::SetupFailed(format!(
            "tunnel '{tunnel_name}' was created but its ID could not be determined"
        ))
    })
}

pub async fn ensure_tunnel(tunnel_name: &str) -> Result<String, TunnelError> {
    if let Some(id) = find_tunnel_id(tunnel_name).await? {
        return Ok(id);
    }
    create_tunnel(tunnel_name).await
}

pub fn find_credentials_file(tunnel_id: &str) -> Result<PathBuf, TunnelError> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let path = PathBuf::from(home)
        .join(".cloudflared")
        .join(format!("{tunnel_id}.json"));
    if path.exists() {
        Ok(path)
    } else {
        Err(TunnelError::CredentialsNotFound(
            path.display().to_string(),
        ))
    }
}

pub fn write_config_file(
    tunnel_id: &str,
    credentials_file: &std::path::Path,
    hostname: &str,
    local_port: u16,
    config_path: &std::path::Path,
) -> Result<(), TunnelError> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            TunnelError::SetupFailed(format!(
                "could not create config directory {}: {e}",
                parent.display()
            ))
        })?;
    }

    let content = format!(
        "tunnel: {tunnel_id}\n\
         credentials-file: {creds}\n\
         \n\
         ingress:\n\
         \x20 - hostname: {hostname}\n\
         \x20   service: http://localhost:{local_port}\n\
         \x20 - service: http_status:404\n",
        creds = credentials_file.display(),
    );

    std::fs::write(config_path, content).map_err(|e| {
        TunnelError::SetupFailed(format!(
            "could not write config file {}: {e}",
            config_path.display()
        ))
    })
}

pub async fn ensure_dns_route(
    tunnel_name: &str,
    hostname: &str,
) -> Result<(), TunnelError> {
    let output = Command::new("cloudflared")
        .args([
            "tunnel",
            "route",
            "dns",
            "--overwrite-dns",
            tunnel_name,
            hostname,
        ])
        .output()
        .await
        .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(TunnelError::SetupFailed(format!(
            "could not route DNS for {hostname}: {stderr}"
        )))
    }
}

pub fn config_file_exists(config_path: &std::path::Path) -> bool {
    config_path.exists()
}
