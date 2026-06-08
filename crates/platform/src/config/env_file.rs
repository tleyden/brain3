use std::env;
use std::path::PathBuf;

use brain3_core::domain::errors::ConfigError;
use brain3_core::domain::model::{
    GatewayConfig, HostnameValidationConfig, MCPReverseProxyConfig, OAuthConfig,
};
use brain3_core::ports::config::ConfigPort;

pub struct EnvFileConfigAdapter {
    env_path: Option<PathBuf>,
}

impl EnvFileConfigAdapter {
    pub fn new(env_path: Option<PathBuf>) -> Self {
        Self { env_path }
    }

    fn load_env_file(&self) {
        if let Some(ref path) = self.env_path {
            let _ = dotenvy::from_path(path);
        } else {
            let _ = dotenvy::dotenv();
        }
    }
}

impl ConfigPort for EnvFileConfigAdapter {
    fn load(&self) -> Result<GatewayConfig, ConfigError> {
        self.load_env_file();

        let port = env_var_or("OAUTH2_GATEWAY_PORT", "8421")
            .parse::<u16>()
            .map_err(|e| ConfigError::Invalid(format!("OAUTH2_GATEWAY_PORT: {e}")))?;

        let expected_host = resolve_expected_host()?;
        let enforce_hostname = env_bool("OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK", true);

        Ok(GatewayConfig {
            port,
            host: "127.0.0.1".to_string(),
            oauth: OAuthConfig {
                client_id: env_var_or("OAUTH2_GATEWAY_CLIENT_ID", "oauth2-gateway-client"),
                client_secret: env_var_or("OAUTH2_GATEWAY_CLIENT_SECRET", ""),
                access_token: env_var_or("OAUTH2_GATEWAY_ACCESS_TOKEN", ""),
                pkce_required: env_bool("OAUTH2_PKCE_REQUIRED", true),
                username: env_var_or("USERNAME", ""),
                password: env_var_or("PASSWORD", ""),
            },
            mcp_reverse_proxy: MCPReverseProxyConfig {
                mcp_upstream_url: env_var_or(
                    "OAUTH2_GATEWAY_MCP_UPSTREAM_URL",
                    "http://127.0.0.1:8420",
                ),
                upstream_secret_file: PathBuf::from(env_var_or(
                    "OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE",
                    "/tmp/brain3-mcp-upstream-secret",
                )),
            },
            hostname_validation: HostnameValidationConfig {
                expected_host,
                enforce: enforce_hostname,
            },
        })
    }
}

fn env_var_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(val) => !["0", "false", "no", "off"]
            .contains(&val.trim().to_lowercase().as_str()),
        Err(_) => default,
    }
}

fn normalize_hostname(value: &str) -> String {
    value.trim().trim_matches('.').to_lowercase()
}

fn named_tunnel_host() -> Option<String> {
    let tunnel_name = normalize_hostname(&env_var_or("CF_TUNNEL_NAME", ""));
    let domain = normalize_hostname(&env_var_or("CF_DOMAIN", ""));
    if tunnel_name.is_empty() || domain.is_empty() {
        return None;
    }
    Some(format!("{tunnel_name}.{domain}"))
}

fn direct_public_origin_hostname() -> Option<String> {
    let hostname = normalize_hostname(&env_var_or("DIRECT_PUBLIC_ORIGIN_HOSTNAME", ""));
    if hostname.is_empty() {
        None
    } else {
        Some(hostname)
    }
}

fn resolve_expected_host() -> Result<Option<String>, ConfigError> {
    let named = named_tunnel_host();
    let direct = direct_public_origin_hostname();

    if named.is_some() && direct.is_some() {
        return Err(ConfigError::Conflict(
            "Both named Cloudflare tunnel hostname settings (CF_TUNNEL_NAME and CF_DOMAIN) \
             and DIRECT_PUBLIC_ORIGIN_HOSTNAME are set. Choose only one public hostname configuration."
                .into(),
        ));
    }

    Ok(named.or(direct))
}
