use std::path::{Path, PathBuf};

use brain3_core::domain::errors::TunnelError;
use brain3_core::domain::model::TunnelConfig;
use brain3_core::ports::tunnel::{TunnelInfo, TunnelPort};

use super::{CloudflareNamedTunnelAdapter, CloudflareQuickTunnelAdapter};

pub async fn start_tunnel(
    config: &TunnelConfig,
    pid_file: PathBuf,
) -> Result<(Box<dyn TunnelPort>, TunnelInfo), TunnelError> {
    let adapter: Box<dyn TunnelPort> = match config {
        TunnelConfig::CloudflareQuick { local_port } => {
            Box::new(CloudflareQuickTunnelAdapter::new(*local_port, pid_file))
        }
        TunnelConfig::CloudflareNamed {
            tunnel_name,
            domain,
            config_file,
            local_port,
        } => {
            validate_named_tunnel_config_port(config_file, *local_port)?;
            Box::new(CloudflareNamedTunnelAdapter::new(
                tunnel_name,
                domain,
                config_file.clone(),
                pid_file,
            ))
        }
    };
    let info = adapter.start().await?;
    Ok((adapter, info))
}

/// Parses the cloudflare named tunnel YAML and checks that at least one non-catch-all
/// ingress rule targets `expected_port`. Returns `TunnelError::PortMismatch` if a real
/// service URL is found pointing to a different port.
fn validate_named_tunnel_config_port(
    config_file: &Path,
    expected_port: u16,
) -> Result<(), TunnelError> {
    let content = std::fs::read_to_string(config_file).map_err(|e| {
        TunnelError::Other(format!(
            "failed to read tunnel config {}: {e}",
            config_file.display()
        ))
    })?;

    let mut found_service = false;
    for line in content.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("service:") else {
            continue;
        };
        let url = rest.trim();
        if url.starts_with("http_status:") {
            continue;
        }
        found_service = true;

        // Extract port from "http[s]://host:PORT[/path]" by splitting on ':' from the right.
        if let Some(after_last_colon) = url.rsplitn(2, ':').next() {
            let port_str = after_last_colon.split('/').next().unwrap_or(after_last_colon);
            if let Ok(config_port) = port_str.parse::<u16>() {
                if config_port == expected_port {
                    return Ok(());
                }
                return Err(TunnelError::PortMismatch {
                    config_port,
                    gateway_port: expected_port,
                    config_file: config_file.display().to_string(),
                });
            }
        }
    }

    if !found_service {
        tracing::warn!(
            config_file = %config_file.display(),
            "no 'service:' lines found in tunnel config — skipping port validation"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write_tunnel_config(service_url: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "tunnel: test-uuid").unwrap();
        writeln!(f, "ingress:").unwrap();
        writeln!(f, "  - hostname: brain3.example.dev").unwrap();
        writeln!(f, "    service: {service_url}").unwrap();
        writeln!(f, "  - service: http_status:404").unwrap();
        f
    }

    #[test]
    fn matching_port_passes() {
        let f = write_tunnel_config("http://localhost:2763");
        assert!(validate_named_tunnel_config_port(f.path(), 2763).is_ok());
    }

    #[test]
    fn mismatched_port_returns_error() {
        let f = write_tunnel_config("http://localhost:8521");
        let err = validate_named_tunnel_config_port(f.path(), 2763).unwrap_err();
        assert!(matches!(
            err,
            TunnelError::PortMismatch {
                config_port: 8521,
                gateway_port: 2763,
                ..
            }
        ));
    }

    #[test]
    fn only_catch_all_no_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "ingress:").unwrap();
        writeln!(f, "  - service: http_status:404").unwrap();
        assert!(validate_named_tunnel_config_port(f.path(), 2763).is_ok());
    }
}
