use std::env;
use std::path::{Path, PathBuf};

use brain3_core::application::first_run_setup::CURRENT_RELEASE;
use brain3_core::domain::errors::ConfigError;
use brain3_core::domain::model::{
    AccessMode, ContainerNetworkIsolationStrategy, ContainerRuntime, ContainerStartupConfig,
    GatewayConfig, HostnameValidationConfig, LocalMcpConfig, MCPReverseProxyConfig, OAuthConfig,
    TunnelConfig,
};
use rand::RngExt;
const DEFAULT_ACCESS_TOKEN_LIFETIME_SECS: u64 = 3600;
const DEFAULT_REFRESH_TOKEN_LIFETIME_SECS: u64 = 90 * 24 * 60 * 60;
const DEFAULT_LOCAL_MCP_PORT: u16 = 2764;
use brain3_core::ports::config::ConfigPort;

pub struct EnvFileConfigAdapter {
    env_path: Option<PathBuf>,
    token_db_home_override: Option<PathBuf>,
}

impl EnvFileConfigAdapter {
    pub fn new(env_path: Option<PathBuf>) -> Self {
        Self {
            env_path,
            token_db_home_override: None,
        }
    }

    pub fn with_token_db_home_override(
        env_path: Option<PathBuf>,
        token_db_home_override: Option<PathBuf>,
    ) -> Self {
        Self {
            env_path,
            token_db_home_override,
        }
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

        let port = env_var_or("B3_OAUTH2_GATEWAY_PORT", "2763")
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
        let access_mode = load_access_mode()?;

        let expected_host = resolve_expected_host()?;
        let enforce_hostname = env_bool("B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK", true);
        let token_db_path = resolve_token_db_path(self.token_db_home_override.as_deref())?;

        let upstream_secret = load_upstream_shared_secret();
        let local_mcp = load_local_mcp_config()?;
        let container = load_container_startup_config(&upstream_secret)?;

        let default_upstream_url = match &container {
            Some(c) => format!("http://127.0.0.1:{}", c.host_port),
            None => "http://127.0.0.1:2765".to_string(),
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
                upstream_secret,
            },
            hostname_validation: HostnameValidationConfig {
                expected_host,
                enforce: enforce_hostname,
            },
            access_mode,
            local_mcp,
            container,
            tunnel,
        })
    }
}

fn env_var_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn load_access_mode() -> Result<AccessMode, ConfigError> {
    match env::var("B3_ACCESS_MODE").as_deref() {
        Ok("local") => Ok(AccessMode::Local),
        Ok("remote") => Ok(AccessMode::Remote),
        Ok("both") | Err(_) => Ok(AccessMode::Both),
        Ok(other) => Err(ConfigError::Invalid(format!(
            "B3_ACCESS_MODE must be 'local', 'remote', or 'both'; got '{other}'"
        ))),
    }
}

fn resolve_token_db_path(token_db_home_override: Option<&Path>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = env::var_os("B3_TOKEN_DB_PATH").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    if let Some(root) = token_db_home_override {
        return Ok(root.join("brain3.db"));
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
                "resolved container image: could not derive container name from '{image}'"
            ))
        })?;

    let without_digest = last_segment.split('@').next().unwrap_or(last_segment);
    let name = without_digest.split(':').next().unwrap_or(without_digest);

    if name.is_empty() {
        return Err(ConfigError::Invalid(format!(
            "resolved container image: could not derive container name from '{image}'"
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

fn load_local_mcp_config() -> Result<Option<LocalMcpConfig>, ConfigError> {
    let port = env::var("B3_LOCAL_MCP_PORT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|e| ConfigError::Invalid(format!("B3_LOCAL_MCP_PORT: {e}")))
        })
        .transpose()?;

    let bearer_token = env::var("LOCAL_GATEWAY_MCP_BEARER_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    match (port, bearer_token) {
        (None, None) => Ok(None),
        (Some(port), Some(bearer_token)) => Ok(Some(LocalMcpConfig { port, bearer_token })),
        (None, Some(bearer_token)) => Ok(Some(LocalMcpConfig {
            port: DEFAULT_LOCAL_MCP_PORT,
            bearer_token,
        })),
        (Some(_), None) => Err(ConfigError::Missing(
            "LOCAL_GATEWAY_MCP_BEARER_TOKEN is required when B3_LOCAL_MCP_PORT is set".into(),
        )),
    }
}

fn load_upstream_shared_secret() -> String {
    if let Some(secret) = env::var("B3_UPSTREAM_SHARED_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        tracing::info!("using pinned MCP upstream shared secret from B3_UPSTREAM_SHARED_SECRET");
        return secret;
    }

    let secret: String = rand::rng()
        .sample_iter(rand::distr::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    tracing::info!("generated in-memory MCP upstream shared secret for this gateway process");
    secret
}

fn load_container_startup_config(
    upstream_secret: &str,
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

    let repo = require_nonempty_env(
        "B3_CONTAINER_IMAGE_REPO",
        "when B3_CONTAINER_RUNTIME is set",
    )?;
    let image_name = repo.rsplit('/').next().unwrap_or(repo.as_str());
    if image_name.contains(':') {
        return Err(ConfigError::Invalid(format!(
            "B3_CONTAINER_IMAGE_REPO must not include a tag (found ':'). \
             Use B3_CONTAINER_IMAGE_TAG to pin a version, or leave it empty \
             to automatically use the version matching this binary ({CURRENT_RELEASE})"
        )));
    }
    let tag_override = env_var_or("B3_CONTAINER_IMAGE_TAG", "");
    if tag_override.contains(':') {
        return Err(ConfigError::Invalid(
            "B3_CONTAINER_IMAGE_TAG must not include a colon (':'). \
             Provide only the tag portion, e.g. v0.1.7 or pr-123."
                .to_string(),
        ));
    }
    let tag = if tag_override.trim().is_empty() {
        CURRENT_RELEASE.to_string()
    } else {
        tag_override.trim().to_string()
    };
    let image = format!("{repo}:{tag}");

    let container_name = {
        let override_name = env_var_or("B3_CONTAINER_NAME", "");
        if !override_name.trim().is_empty() {
            override_name.trim().to_string()
        } else {
            derive_container_name_from_image(&image)?
        }
    };

    let network_name = {
        let override_name = env_var_or("B3_CONTAINER_NETWORK_NAME", "");
        if !override_name.trim().is_empty() {
            override_name.trim().to_string()
        } else {
            "brain3-mcp-net".to_string()
        }
    };

    let host_port = env_var_or("B3_CONTAINER_HOST_PORT", "2765")
        .parse::<u16>()
        .map_err(|e| ConfigError::Invalid(format!("B3_CONTAINER_HOST_PORT: {e}")))?;

    let container_port = env_var_or("B3_CONTAINER_MCP_PORT", "2765")
        .parse::<u16>()
        .map_err(|e| ConfigError::Invalid(format!("B3_CONTAINER_MCP_PORT: {e}")))?;
    let network_isolated = env_bool("B3_CONTAINER_INTERNAL_NETWORK_ISOLATION", true);
    validate_network_isolation_support(runtime, network_isolated)?;

    let isolation_strategy = if network_isolated {
        let strategy_str = env_var_or("B3_CONTAINER_NETWORK_ISOLATION_STRATEGY", "");
        let strategy = match strategy_str.trim() {
            "" => match runtime {
                ContainerRuntime::Docker => ContainerNetworkIsolationStrategy::DiscoverContainerIp,
                ContainerRuntime::MacOSContainer => {
                    ContainerNetworkIsolationStrategy::PublishToLoopback
                }
            },
            "discover-container-ip" => ContainerNetworkIsolationStrategy::DiscoverContainerIp,
            "publish-to-loopback" => ContainerNetworkIsolationStrategy::PublishToLoopback,
            other => {
                return Err(ConfigError::Invalid(format!(
                    "B3_CONTAINER_NETWORK_ISOLATION_STRATEGY: unknown value '{other}'; \
                     expected 'discover-container-ip' or 'publish-to-loopback'"
                )))
            }
        };
        Some(strategy)
    } else {
        None
    };

    let dev_mount_source = env::var("B3_DEV_CONTAINER_MOUNT_SOURCE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from);

    let mcp_log_level = env::var("B3_VAULT_MCP_LOG_LEVEL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Ok(Some(ContainerStartupConfig {
        runtime,
        image,
        container_name,
        network_name,
        vault_path: PathBuf::from(vault_path_str),
        upstream_secret: upstream_secret.to_string(),
        host_port,
        container_port,
        isolation_strategy,
        dev_mount_source,
        mcp_log_level,
    }))
}

fn validate_network_isolation_support(
    runtime: ContainerRuntime,
    network_isolated: bool,
) -> Result<(), ConfigError> {
    if !network_isolated {
        return Ok(());
    }

    if matches!(runtime, ContainerRuntime::Docker) && env::consts::OS == "macos" {
        return Err(ConfigError::Invalid(
            "B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true is not supported with B3_CONTAINER_RUNTIME=docker on macos. For highest security, use the native macos-container runtime instead of Docker. Otherwise set B3_CONTAINER_RUNTIME=macos-container or B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false. Linux Docker is supported."
                .into(),
        ));
    }

    Ok(())
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

    // Default: tunneling is disabled unless explicitly enabled.
    let quick_default = env_bool("B3_CF_QUICK_TUNNEL", false);
    if quick_default {
        tracing::info!("B3_CF_QUICK_TUNNEL=true — using Cloudflare quick tunnel");
        return Ok(Some(TunnelConfig::CloudflareQuick {
            local_port: gateway_port,
        }));
    }

    tracing::info!(
        "B3_CF_QUICK_TUNNEL is not enabled and no named tunnel vars set — no tunnel configured"
    );
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{LazyLock, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
    const CONFIG_KEYS: &[&str] = &[
        "B3_OAUTH2_GATEWAY_CLIENT_SECRET",
        "B3_USERNAME",
        "B3_PASSWORD",
        "B3_TOKEN_DB_PATH",
        "B3_UPSTREAM_SHARED_SECRET",
        "B3_CONTAINER_RUNTIME",
        "B3_VAULT_PATH",
        "B3_CONTAINER_IMAGE_REPO",
        "B3_CONTAINER_IMAGE_TAG",
        "B3_CONTAINER_INTERNAL_NETWORK_ISOLATION",
        "B3_CONTAINER_NETWORK_ISOLATION_STRATEGY",
        "B3_CONTAINER_HOST_PORT",
        "B3_CONTAINER_MCP_PORT",
        "B3_ACCESS_MODE",
        "B3_LOCAL_MCP_PORT",
        "LOCAL_GATEWAY_MCP_BEARER_TOKEN",
        "B3_CF_QUICK_TUNNEL",
    ];

    fn with_clean_config_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved: Vec<(&str, Option<String>)> = CONFIG_KEYS
            .iter()
            .map(|key| (*key, env::var(key).ok()))
            .collect();

        for key in CONFIG_KEYS {
            env::remove_var(key);
        }

        let result = f();

        for key in CONFIG_KEYS {
            env::remove_var(key);
        }
        for (key, value) in saved {
            if let Some(value) = value {
                env::set_var(key, value);
            }
        }

        result
    }

    fn write_test_env_file(contents: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = env::temp_dir().join(format!("brain3-env-file-test-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        fs::write(&env_path, contents).unwrap();
        env_path
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn load_rejects_internal_network_isolation_for_docker_on_macos() {
        with_clean_config_env(|| {
            let vault_dir = env::temp_dir().join("brain3-config-test-vault-macos");
            fs::create_dir_all(&vault_dir).unwrap();
            let token_db = env::temp_dir().join("brain3-config-test-macos.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_CF_QUICK_TUNNEL=false\n\
                 B3_CONTAINER_RUNTIME=docker\n\
                 B3_VAULT_PATH={}\n\
                 B3_CONTAINER_IMAGE_REPO=ghcr.io/tleyden/brain3-mcp-vault-tools\n\
                 B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true\n",
                token_db.display(),
                vault_dir.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let err = adapter
                .load()
                .expect_err("expected invalid macos docker config");

            match err {
                ConfigError::Invalid(message) => {
                    assert!(message.contains("B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true"));
                    assert!(message.contains("B3_CONTAINER_RUNTIME=docker"));
                    assert!(message.contains("macos-container"));
                }
                other => panic!("expected invalid config error, got {other:?}"),
            }
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn load_allows_internal_network_isolation_for_docker_on_linux() {
        with_clean_config_env(|| {
            let vault_dir = env::temp_dir().join("brain3-config-test-vault-linux");
            fs::create_dir_all(&vault_dir).unwrap();
            let token_db = env::temp_dir().join("brain3-config-test-linux.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_CF_QUICK_TUNNEL=false\n\
                 B3_CONTAINER_RUNTIME=docker\n\
                 B3_VAULT_PATH={}\n\
                 B3_CONTAINER_IMAGE_REPO=ghcr.io/tleyden/brain3-mcp-vault-tools\n\
                 B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true\n",
                token_db.display(),
                vault_dir.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let config = adapter
                .load()
                .expect("expected linux docker config to load");

            assert_eq!(
                config.container.as_ref().map(|c| c.runtime),
                Some(ContainerRuntime::Docker)
            );
            assert_eq!(
                config.container.as_ref().map(|c| c.isolation_strategy),
                Some(Some(ContainerNetworkIsolationStrategy::DiscoverContainerIp))
            );
        });
    }

    #[test]
    fn load_resolves_release_tag_when_container_image_tag_is_empty() {
        with_clean_config_env(|| {
            let vault_dir = env::temp_dir().join("brain3-config-test-vault-release-tag");
            fs::create_dir_all(&vault_dir).unwrap();
            let token_db = env::temp_dir().join("brain3-config-test-release-tag.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_CF_QUICK_TUNNEL=false\n\
                 B3_CONTAINER_RUNTIME=macos-container\n\
                 B3_VAULT_PATH={}\n\
                 B3_CONTAINER_IMAGE_REPO=ghcr.io/tleyden/brain3-mcp-vault-tools\n\
                 B3_CONTAINER_IMAGE_TAG=\n",
                token_db.display(),
                vault_dir.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let config = adapter.load().expect("expected config to load");
            let expected_image =
                format!("ghcr.io/tleyden/brain3-mcp-vault-tools:{CURRENT_RELEASE}");

            assert_eq!(
                config.container.as_ref().map(|c| c.image.as_str()),
                Some(expected_image.as_str())
            );
        });
    }

    #[test]
    fn load_uses_explicit_container_image_tag_when_present() {
        with_clean_config_env(|| {
            let vault_dir = env::temp_dir().join("brain3-config-test-vault-explicit-tag");
            fs::create_dir_all(&vault_dir).unwrap();
            let token_db = env::temp_dir().join("brain3-config-test-explicit-tag.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_CF_QUICK_TUNNEL=false\n\
                 B3_CONTAINER_RUNTIME=macos-container\n\
                 B3_VAULT_PATH={}\n\
                 B3_CONTAINER_IMAGE_REPO=ghcr.io/tleyden/brain3-mcp-vault-tools\n\
                 B3_CONTAINER_IMAGE_TAG=v0.1.7\n",
                token_db.display(),
                vault_dir.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let config = adapter.load().expect("expected config to load");

            assert_eq!(
                config.container.as_ref().map(|c| c.image.as_str()),
                Some("ghcr.io/tleyden/brain3-mcp-vault-tools:v0.1.7")
            );
        });
    }

    #[test]
    fn load_allows_container_image_repo_with_registry_port() {
        with_clean_config_env(|| {
            let vault_dir = env::temp_dir().join("brain3-config-test-vault-registry-port");
            fs::create_dir_all(&vault_dir).unwrap();
            let token_db = env::temp_dir().join("brain3-config-test-registry-port.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_CF_QUICK_TUNNEL=false\n\
                 B3_CONTAINER_RUNTIME=macos-container\n\
                 B3_VAULT_PATH={}\n\
                 B3_CONTAINER_IMAGE_REPO=localhost:5000/tleyden/brain3-mcp-vault-tools\n\
                 B3_CONTAINER_IMAGE_TAG=v0.1.7\n",
                token_db.display(),
                vault_dir.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let config = adapter.load().expect("expected config to load");

            assert_eq!(
                config.container.as_ref().map(|c| c.image.as_str()),
                Some("localhost:5000/tleyden/brain3-mcp-vault-tools:v0.1.7")
            );
        });
    }

    #[test]
    fn load_rejects_container_image_repo_that_includes_a_tag() {
        with_clean_config_env(|| {
            let vault_dir = env::temp_dir().join("brain3-config-test-vault-invalid-repo");
            fs::create_dir_all(&vault_dir).unwrap();
            let token_db = env::temp_dir().join("brain3-config-test-invalid-repo.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_CF_QUICK_TUNNEL=false\n\
                 B3_CONTAINER_RUNTIME=macos-container\n\
                 B3_VAULT_PATH={}\n\
                 B3_CONTAINER_IMAGE_REPO=ghcr.io/tleyden/brain3-mcp-vault-tools:v0.1.7\n\
                 B3_CONTAINER_IMAGE_TAG=v0.1.8\n",
                token_db.display(),
                vault_dir.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let err = adapter.load().expect_err("expected invalid config");

            match err {
                ConfigError::Invalid(message) => {
                    assert!(message.contains("B3_CONTAINER_IMAGE_REPO must not include a tag"));
                    assert!(message.contains("B3_CONTAINER_IMAGE_TAG"));
                }
                other => panic!("expected invalid config error, got {other:?}"),
            }
        });
    }

    #[test]
    fn load_defaults_quick_tunnel_to_disabled() {
        with_clean_config_env(|| {
            let token_db = env::temp_dir().join("brain3-config-test-no-tunnel.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n",
                token_db.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let config = adapter.load().expect("expected config to load");

            assert_eq!(config.access_mode, AccessMode::Both);
            assert!(config.tunnel.is_none(), "quick tunnel should be opt-in");
        });
    }

    #[test]
    fn load_parses_explicit_local_access_mode() {
        with_clean_config_env(|| {
            let token_db = env::temp_dir().join("brain3-config-test-local-access-mode.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_ACCESS_MODE=local\n\
                 B3_LOCAL_MCP_PORT=2764\n\
                 LOCAL_GATEWAY_MCP_BEARER_TOKEN=local-token\n",
                token_db.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let config = adapter.load().expect("expected config to load");

            assert_eq!(config.access_mode, AccessMode::Local);
        });
    }

    #[test]
    fn load_rejects_invalid_access_mode() {
        with_clean_config_env(|| {
            let token_db = env::temp_dir().join("brain3-config-test-invalid-access-mode.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_ACCESS_MODE=invalid\n",
                token_db.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let err = adapter.load().expect_err("expected invalid access mode");

            match err {
                ConfigError::Invalid(message) => {
                    assert!(message.contains("B3_ACCESS_MODE"));
                    assert!(message.contains("local"));
                    assert!(message.contains("remote"));
                    assert!(message.contains("both"));
                }
                other => panic!("expected invalid config error, got {other:?}"),
            }
        });
    }

    #[test]
    fn load_enables_local_mcp_on_default_port_when_only_token_is_set() {
        with_clean_config_env(|| {
            let token_db = env::temp_dir().join("brain3-config-test-local-mcp-default-port.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 LOCAL_GATEWAY_MCP_BEARER_TOKEN=local-token\n",
                token_db.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let config = adapter.load().expect("expected config to load");

            let local = config
                .local_mcp
                .expect("local MCP config should be enabled");
            assert_eq!(local.port, 2764);
            assert_eq!(local.bearer_token, "local-token");
        });
    }

    #[test]
    fn load_rejects_local_mcp_port_without_token() {
        with_clean_config_env(|| {
            let token_db = env::temp_dir().join("brain3-config-test-local-mcp-missing-token.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_LOCAL_MCP_PORT=2764\n",
                token_db.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let err = adapter
                .load()
                .expect_err("expected missing token to be rejected");

            match err {
                ConfigError::Missing(message) => {
                    assert!(message.contains("LOCAL_GATEWAY_MCP_BEARER_TOKEN"));
                }
                other => panic!("expected missing config error, got {other:?}"),
            }
        });
    }

    #[test]
    fn load_uses_host_upstream_secret_when_set() {
        with_clean_config_env(|| {
            let token_db = env::temp_dir().join("brain3-config-test-host-upstream-secret.db");
            let env_path = write_test_env_file(&format!(
                "B3_OAUTH2_GATEWAY_CLIENT_SECRET=test-secret\n\
                 B3_USERNAME=test-user\n\
                 B3_PASSWORD=test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_UPSTREAM_SHARED_SECRET=pinned-upstream-secret\n",
                token_db.display()
            ));

            let adapter = EnvFileConfigAdapter::new(Some(env_path));
            let config = adapter.load().expect("expected config to load");

            assert_eq!(
                config.mcp_reverse_proxy.upstream_secret,
                "pinned-upstream-secret"
            );
        });
    }
}
