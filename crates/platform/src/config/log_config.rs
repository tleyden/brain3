use brain3_core::domain::model::{GatewayConfig, TunnelConfig};

pub fn log_startup_config(config: &GatewayConfig) {
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
        upstream_url = %config.mcp_reverse_proxy.mcp_upstream_url,
        upstream_secret_configured = !config.mcp_reverse_proxy.upstream_secret.is_empty(),
        expected_host = ?config.hostname_validation.expected_host,
        enforce_hostname = config.hostname_validation.enforce,
        local_mcp = ?config.local_mcp.as_ref().map(|cfg| format!(
            "port={} token_configured={}",
            cfg.port,
            !cfg.bearer_token.is_empty()
        )),
        container = ?config.container.as_ref().map(|c| format!(
            "{:?} image={} name={} vault={} port={}",
            c.runtime, c.image, c.container_name, c.vault_path.display(), c.host_port
        )),
        tunnel = ?config.tunnel.as_ref().map(tunnel_summary),
        "startup config"
    );
    tracing::info!(
        token_db_path = %config.token_db_path.display(),
        "using SQLite token database"
    );
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
