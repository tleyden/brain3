use brain3_core::domain::model::{GatewayConfig, TunnelConfig, UpstreamTransport};

pub fn log_startup_config(config: &GatewayConfig) {
    let upstream_secret_dir = config
        .mcp_reverse_proxy
        .upstream_secret_file
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unknown>".into());

    tracing::info!(
        port = config.port,
        token_db_path = %config.token_db_path.display(),
        client_id = %config.oauth.client_id,
        client_secret = mask(&config.oauth.client_secret),
        access_token_lifetime_secs = config.oauth.access_token_lifetime_secs,
        refresh_token_lifetime_secs = config.oauth.refresh_token_lifetime_secs,
        username = %config.oauth.username,
        password = mask(&config.oauth.password),
        pkce_required = config.oauth.pkce_required,
        upstream = %upstream_summary(&config.mcp_reverse_proxy.upstream),
        upstream_secret_file = %config.mcp_reverse_proxy.upstream_secret_file.display(),
        upstream_secret_dir = %upstream_secret_dir,
        expected_host = ?config.hostname_validation.expected_host,
        enforce_hostname = config.hostname_validation.enforce,
        container = ?config.container.as_ref().map(|c| format!(
            "{:?} image={} name={} vault={} port={}",
            c.runtime, c.image, c.container_name, c.vault_path.display(), c.host_port
        )),
        tunnel = ?config.tunnel.as_ref().map(tunnel_summary),
        "startup config"
    );
}

fn upstream_summary(t: &UpstreamTransport) -> String {
    match t {
        UpstreamTransport::Http { url } => format!("tcp:{url}"),
        UpstreamTransport::UnixSocket { socket_path } => {
            format!("unix:{}", socket_path.display())
        }
    }
}

fn tunnel_summary(t: &TunnelConfig) -> String {
    match t {
        TunnelConfig::CloudflareQuick { local_port } => {
            format!("cloudflare-quick port={local_port}")
        }
        TunnelConfig::CloudflareNamed {
            tunnel_name,
            domain,
            config_file,
            ..
        } => {
            format!(
                "cloudflare-named {tunnel_name}.{domain} config={}",
                config_file.display()
            )
        }
    }
}

fn mask(s: &str) -> &str {
    if s.is_empty() {
        "[not set]"
    } else {
        "****"
    }
}
