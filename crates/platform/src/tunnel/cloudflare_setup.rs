use std::path::PathBuf;

use brain3_core::domain::errors::TunnelError;
use tokio::process::Command;

pub async fn check_cloudflared_installed() -> bool {
    Command::new("which")
        .arg("cloudflared")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns true if `cloudflared tunnel list` succeeds (i.e. user is logged in).
pub async fn check_cloudflared_logged_in() -> bool {
    Command::new("cloudflared")
        .args(["tunnel", "list"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Spawns `cloudflared tunnel login` and returns its stdout+stderr combined as a stream.
/// The caller is responsible for driving the child to completion and streaming output to the TUI.
pub async fn spawn_cloudflared_login() -> Result<tokio::process::Child, TunnelError> {
    Command::new("cloudflared")
        .args(["tunnel", "login"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| TunnelError::SpawnFailed(e.to_string()))
}

/// Returns the tunnel ID for `name`, or None if no such tunnel exists.
pub async fn find_tunnel_id(name: &str) -> Result<Option<String>, TunnelError> {
    let out = Command::new("cloudflared")
        .args(["tunnel", "list", "--name", name, "--output", "json"])
        .output()
        .await
        .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(TunnelError::SetupFailed(format!(
            "cloudflared tunnel list failed: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Output is a JSON array of tunnel objects; an empty array means not found.
    let trimmed = stdout.trim();
    if trimmed == "null" || trimmed == "[]" {
        return Ok(None);
    }

    // Parse the first element's "id" field with minimal JSON handling.
    if let Some(id) = parse_first_tunnel_id(trimmed) {
        Ok(Some(id))
    } else {
        Ok(None)
    }
}

fn parse_first_tunnel_id(json: &str) -> Option<String> {
    // cloudflared outputs pretty-printed JSON so the value may be `"id": "` or `"id":"`.
    let start = json
        .find("\"id\": \"")
        .map(|i| i + 7)
        .or_else(|| json.find("\"id\":\"").map(|i| i + 6))?;
    let end = json[start..].find('"')? + start;
    Some(json[start..end].to_string())
}

/// Creates a named tunnel and returns its ID.
pub async fn create_tunnel(name: &str) -> Result<String, TunnelError> {
    let out = Command::new("cloudflared")
        .args(["tunnel", "create", "--output", "json", name])
        .output()
        .await
        .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(TunnelError::SetupFailed(format!(
            "cloudflared tunnel create failed: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_first_tunnel_id(&stdout).ok_or_else(|| {
        TunnelError::SetupFailed(format!("could not parse tunnel ID from: {stdout}"))
    })
}

/// Returns the path to the credentials JSON file for a tunnel ID, if it exists.
pub fn find_credentials_file(tunnel_id: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(format!("{home}/.cloudflared/{tunnel_id}.json"));
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Writes the cloudflared named tunnel config YAML to `config_path`.
pub fn write_config_file(
    config_path: &PathBuf,
    tunnel_id: &str,
    credentials_file: &PathBuf,
    tunnel_name: &str,
    domain: &str,
    local_port: u16,
) -> Result<(), TunnelError> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TunnelError::SetupFailed(format!("could not create config dir: {e}")))?;
    }

    let hostname = format!("{tunnel_name}.{domain}");
    let content = format!(
        "tunnel: {tunnel_id}\ncredentials-file: {creds}\n\ningress:\n  - hostname: {hostname}\n    service: http://localhost:{local_port}\n  - service: http_status:404\n",
        creds = credentials_file.display(),
    );

    std::fs::write(config_path, content)
        .map_err(|e| TunnelError::SetupFailed(format!("could not write config file: {e}")))?;

    Ok(())
}

/// Runs `cloudflared tunnel route dns --overwrite-dns <name> <hostname>`.
pub async fn ensure_dns_route(name: &str, hostname: &str) -> Result<(), TunnelError> {
    let status = Command::new("cloudflared")
        .args(["tunnel", "route", "dns", "--overwrite-dns", name, hostname])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| TunnelError::SpawnFailed(e.to_string()))?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(TunnelError::SetupFailed(format!(
            "cloudflared tunnel route dns failed: {stderr}"
        )));
    }
    Ok(())
}
