use brain3_core::domain::model::GatewayConfig;

pub fn log_startup_config(config: &GatewayConfig) {
    tracing::info!(
        port = config.port,
        client_id = %config.oauth.client_id,
        client_secret = mask(&config.oauth.client_secret),
        access_token = mask(&config.oauth.access_token),
        username = %config.oauth.username,
        password = mask(&config.oauth.password),
        pkce_required = config.oauth.pkce_required,
        upstream_url = %config.mcp_reverse_proxy.mcp_upstream_url,
        upstream_secret_file = %config.mcp_reverse_proxy.upstream_secret_file.display(),
        expected_host = ?config.hostname_validation.expected_host,
        enforce_hostname = config.hostname_validation.enforce,
        container = ?config.container.as_ref().map(|c| format!(
            "{:?} image={} name={} vault={} port={}",
            c.runtime, c.image, c.container_name, c.vault_path.display(), c.host_port
        )),
        "startup config"
    );
}

fn mask(s: &str) -> &str {
    if s.is_empty() { "[not set]" } else { "****" }
}
