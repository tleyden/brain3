use std::env;
use std::path::PathBuf;

use brain3_core::domain::errors::ConfigError;
use brain3_core::domain::model::{
    ContainerRuntime, ContainerStartupConfig, GatewayConfig, HostnameValidationConfig,
    MCPReverseProxyConfig, OAuthConfig, TunnelConfig,
};
use brain3_core::domain::oauth::{
    DEFAULT_ACCESS_TOKEN_LIFETIME_SECS, DEFAULT_REFRESH_TOKEN_LIFETIME_SECS,
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
            match dotenvy::from_path(path) {
                Ok(_) => tracing::info!(path = %path.display(), "loaded env file"),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to load env file")
                }
            }
        } else {
            match dotenvy::dotenv() {
                Ok(path) => tracing::info!(path = %path.display(), "loaded .env file"),
                Err(_) => {
                    tracing::warn!("no .env file found; falling back to environment variables")
                }
            }
        }
    }
}

impl ConfigPort for EnvFileConfigAdapter {
    fn load(&self) -> Result<GatewayConfig, ConfigError> {
        self.load_env_file();

        let port = env_var_or("B3_OAUTH2_GATEWAY_PORT", "8421")
            .parse::<u16>()
            .map_err(|e| ConfigError::Invalid(format!("B3_OAUTH2_GATEWAY_PORT: {e}")))?;
        let access_token_lifetime_secs = env_var_or(
            "B3_OAUTH2_ACCESS_TOKEN_LIFETIME_SECS",
            &DEFAULT_ACCESS_TOKEN_LIFETIME_SECS.to_string(),
        )
        .parse::<u64>()
        .map_err(|e| ConfigError::Invalid(format!("B3_OAUTH2_ACCESS_TOKEN_LIFETIME_SECS: {e}")))?;
        if access_token_lifetime_secs == 0 {
            return Err(ConfigError::Invalid(
                "B3_OAUTH2_ACCESS_TOKEN_LIFETIME_SECS must be greater than 0".into(),
            ));
        }
        let refresh_token_lifetime_secs = env_var_or(
            "B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS",
            &DEFAULT_REFRESH_TOKEN_LIFETIME_SECS.to_string(),
        )
        .parse::<u64>()
        .map_err(|e| ConfigError::Invalid(format!("B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS: {e}")))?;
        if refresh_token_lifetime_secs == 0 {
            return Err(ConfigError::Invalid(
                "B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS must be greater than 0".into(),
            ));
        }

        let expected_host = resolve_expected_host()?;
        let enforce_hostname = env_bool("B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK", true);
        let token_db_path = resolve_token_db_path()?;

        let upstream_secret_file = PathBuf::from(env_var_or(
            "B3_OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE",
            "/tmp/brain3-mcp-upstream-secret",
        ));

        let container = load_container_startup_config(&upstream_secret_file)?;

        let default_upstream_url = match &container {
            Some(c) => format!("http://127.0.0.1:{}", c.host_port),
            None => "http://127.0.0.1:8420".to_string(),
        };

        let mut missing = Vec::new();
        let client_secret = require_nonempty("B3_OAUTH2_GATEWAY_CLIENT_SECRET", &mut missing);
        let username = require_nonempty("B3_USERNAME", &mut missing);
        let password = require_nonempty("B3_PASSWORD", &mut missing);
        if !missing.is_empty() {
            return Err(ConfigError::Missing(format!(
                "required env vars not set: {}",
                missing.join(", ")
            )));
        }

        let tunnel = load_tunnel_config(port)?;

        Ok(GatewayConfig {
            port,
            host: "127.0.0.1".to_string(),
            token_db_path,
            oauth: OAuthConfig {
                client_id: env_var_or("B3_OAUTH2_GATEWAY_CLIENT_ID", "brain3-oauth2-client"),
                client_secret,
                access_token_lifetime_secs,
                refresh_token_lifetime_secs,
                pkce_required: env_bool("B3_OAUTH2_PKCE_REQUIRED", true),
                username,
                password,
            },
            mcp_reverse_proxy: MCPReverseProxyConfig {
                mcp_upstream_url: env_var_or(
                    "B3_OAUTH2_GATEWAY_MCP_UPSTREAM_URL",
                    &default_upstream_url,
                ),
                upstream_secret_file,
            },
            hostname_validation: HostnameValidationConfig {
                expected_host,
                enforce: enforce_hostname,
            },
            container,
            tunnel,
        })
    }
}

fn env_var_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn resolve_token_db_path() -> Result<PathBuf, ConfigError> {
    if let Some(path) = env::var_os("B3_TOKEN_DB_PATH").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    if let Some(root) = env::var_os("B3_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(root).join("brain3.db"));
    }

    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ConfigError::Missing(
                "HOME environment variable is not set; cannot resolve default token database path"
                    .into(),
            )
        })?;

    Ok(PathBuf::from(home).join(".brain3").join("brain3.db"))
}

fn require_nonempty(name: &str, errors: &mut Vec<String>) -> String {
    match env::var(name) {
        Ok(val) if !val.trim().is_empty() => val,
        _ => {
            errors.push(name.to_string());
            String::new()
        }
    }
}

fn require_nonempty_env(name: &str, context: &str) -> Result<String, ConfigError> {
    match env::var(name) {
        Ok(val) if !val.trim().is_empty() => Ok(val),
        _ => Err(ConfigError::Missing(format!(
            "{name} is required {context}"
        ))),
    }
}

fn derive_container_name_from_image(image: &str) -> Result<String, ConfigError> {
    let last_segment = image
        .trim()
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| {
            ConfigError::Invalid(format!(
                "B3_CONTAINER_IMAGE: could not derive container name from '{image}'"
            ))
        })?;

    let without_digest = last_segment.split('@').next().unwrap_or(last_segment);
    let name = without_digest.split(':').next().unwrap_or(without_digest);

    if name.is_empty() {
        return Err(ConfigError::Invalid(format!(
            "B3_CONTAINER_IMAGE: could not derive container name from '{image}'"
        )));
    }

    Ok(name.to_string())
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(val) => !["0", "false", "no", "off"].contains(&val.trim().to_lowercase().as_str()),
        Err(_) => default,
    }
}

fn normalize_hostname(value: &str) -> String {
    value.trim().trim_matches('.').to_lowercase()
}

fn named_tunnel_host() -> Option<String> {
    let tunnel_name = normalize_hostname(&env_var_or("B3_CF_TUNNEL_NAME", ""));
    let domain = normalize_hostname(&env_var_or("B3_CF_DOMAIN", ""));
    if tunnel_name.is_empty() || domain.is_empty() {
        return None;
    }
    Some(format!("{tunnel_name}.{domain}"))
}

fn direct_public_origin_hostname() -> Option<String> {
    let hostname = normalize_hostname(&env_var_or("B3_DIRECT_PUBLIC_ORIGIN_HOSTNAME", ""));
    if hostname.is_empty() {
        None
    } else {
        Some(hostname)
    }
}

fn load_container_startup_config(
    upstream_secret_file: &PathBuf,
) -> Result<Option<ContainerStartupConfig>, ConfigError> {
    let runtime_str = env_var_or("B3_CONTAINER_RUNTIME", "");
    let runtime_str = runtime_str.trim();
    if runtime_str.is_empty() {
        return Ok(None);
    }

    let runtime = match runtime_str {
        "docker" => ContainerRuntime::Docker,
        "macos-container" => ContainerRuntime::MacOSContainer,
        other => {
            return Err(ConfigError::Invalid(format!(
            "B3_CONTAINER_RUNTIME: unknown value '{other}'; expected 'docker' or 'macos-container'"
        )))
        }
    };

    let vault_path_str = require_nonempty_env("B3_VAULT_PATH", "when B3_CONTAINER_RUNTIME is set")?;

    let image = require_nonempty_env("B3_CONTAINER_IMAGE", "when B3_CONTAINER_RUNTIME is set")?;

    let container_name = {
        let override_name = env_var_or("B3_CONTAINER_NAME", "");
        if !override_name.trim().is_empty() {
            override_name.trim().to_string()
        } else {
            derive_container_name_from_image(&image)?
        }
    };

    let host_port = env_var_or("B3_CONTAINER_HOST_PORT", "8420")
        .parse::<u16>()
        .map_err(|e| ConfigError::Invalid(format!("B3_CONTAINER_HOST_PORT: {e}")))?;

    let container_port = env_var_or("B3_CONTAINER_MCP_PORT", "8420")
        .parse::<u16>()
        .map_err(|e| ConfigError::Invalid(format!("B3_CONTAINER_MCP_PORT: {e}")))?;
    // Disabled by default since this is still experimental
    let network_isolated = env_bool("B3_CONTAINER_INTERNAL_NETWORK_ISOLATION", false);

    let upstream_secret_dir = upstream_secret_file
        .parent()
        .unwrap_or(std::path::Path::new("/tmp"))
        .to_path_buf();

    let dev_mount_source = env::var("B3_DEV_CONTAINER_MOUNT_SOURCE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from);

    Ok(Some(ContainerStartupConfig {
        runtime,
        image,
        container_name,
        vault_path: PathBuf::from(vault_path_str),
        upstream_secret_dir,
        host_port,
        container_port,
        network_isolated,
        dev_mount_source,
    }))
}

fn load_tunnel_config(gateway_port: u16) -> Result<Option<TunnelConfig>, ConfigError> {
    // CF_QUICK_TUNNEL=true takes explicit precedence over named-tunnel vars.
    let quick_explicit = env::var("B3_CF_QUICK_TUNNEL")
        .map(|v| !["0", "false", "no", "off"].contains(&v.trim().to_lowercase().as_str()))
        .unwrap_or(false);

    if quick_explicit {
        tracing::info!(
            "B3_CF_QUICK_TUNNEL=true — using Cloudflare quick tunnel (named tunnel vars ignored)"
        );
        return Ok(Some(TunnelConfig::CloudflareQuick {
            local_port: gateway_port,
        }));
    }

    let tunnel_name = normalize_hostname(&env_var_or("B3_CF_TUNNEL_NAME", ""));
    let domain = normalize_hostname(&env_var_or("B3_CF_DOMAIN", ""));
    let named = !tunnel_name.is_empty() && !domain.is_empty();

    if named {
        let config_file_str = env_var_or("B3_CF_TUNNEL_CONFIG_FILE", "");
        let config_file = if config_file_str.is_empty() {
            PathBuf::from(format!(".cloudflared/{tunnel_name}.yml"))
        } else {
            PathBuf::from(config_file_str)
        };
        tracing::info!(tunnel_name = %tunnel_name, domain = %domain, config_file = %config_file.display(), "using Cloudflare named tunnel");
        return Ok(Some(TunnelConfig::CloudflareNamed {
            tunnel_name,
            domain,
            config_file,
            local_port: gateway_port,
        }));
    }

    // Default: quick tunnel unless explicitly disabled.
    let quick_default = env_bool("B3_CF_QUICK_TUNNEL", true);
    if quick_default {
        tracing::info!("B3_CF_QUICK_TUNNEL defaulting to true — using Cloudflare quick tunnel");
        return Ok(Some(TunnelConfig::CloudflareQuick {
            local_port: gateway_port,
        }));
    }

    tracing::info!("B3_CF_QUICK_TUNNEL=false and no named tunnel vars set — no tunnel configured");
    Ok(None)
}

fn resolve_expected_host() -> Result<Option<String>, ConfigError> {
    // Quick tunnel hostname is ephemeral (changes every restart), so there is no
    // stable expected host to enforce. Named tunnel vars must not bleed through.
    let quick_explicit = env::var("B3_CF_QUICK_TUNNEL")
        .map(|v| !["0", "false", "no", "off"].contains(&v.trim().to_lowercase().as_str()))
        .unwrap_or(false);
    if quick_explicit {
        let named = named_tunnel_host();
        if named.is_some() {
            tracing::warn!(
                named_host = ?named,
                "B3_CF_QUICK_TUNNEL=true overrides B3_CF_TUNNEL_NAME/B3_CF_DOMAIN for tunnel mode; \
                 hostname validation will be disabled (quick tunnel URL is ephemeral). \
                 Remove B3_CF_TUNNEL_NAME/B3_CF_DOMAIN or switch to a named tunnel to enable hostname enforcement."
            );
        }
        return Ok(None);
    }

    let named = named_tunnel_host();
    let direct = direct_public_origin_hostname();

    if named.is_some() && direct.is_some() {
        return Err(ConfigError::Conflict(
            "Both named Cloudflare tunnel hostname settings (B3_CF_TUNNEL_NAME and B3_CF_DOMAIN) \
             and B3_DIRECT_PUBLIC_ORIGIN_HOSTNAME are set. Choose only one public hostname configuration."
                .into(),
        ));
    }

    Ok(named.or(direct))
}
