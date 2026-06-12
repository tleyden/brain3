use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub port: u16,
    pub host: String,
    pub token_db_path: PathBuf,
    pub oauth: OAuthConfig,
    pub mcp_reverse_proxy: MCPReverseProxyConfig,
    pub hostname_validation: HostnameValidationConfig,
    pub container: Option<ContainerStartupConfig>,
    pub tunnel: Option<TunnelConfig>,
}

#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub access_token_lifetime_secs: u64,
    pub pkce_required: bool,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct MCPReverseProxyConfig {
    pub mcp_upstream_url: String,
    pub upstream_secret_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HostnameValidationConfig {
    pub expected_host: Option<String>,
    pub enforce: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerRuntime {
    Docker,
    MacOSContainer,
}

/// Config passed to ContainerPort::run — runtime-agnostic.
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub image: String,
    pub name: String,
    pub port_mappings: Vec<PortMapping>,
    pub env_vars: Vec<(String, String)>,
    pub bind_mounts: Vec<BindMount>,
    /// "uid:gid" string; None means run as container default user.
    pub user: Option<String>,
    pub detach: bool,
    pub remove_on_exit: bool,
    pub workdir: Option<String>,
    /// Override the image's default CMD/entrypoint.
    pub command: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PortMapping {
    pub host_address: String,
    pub host_port: u16,
    pub container_port: u16,
}

#[derive(Debug, Clone)]
pub struct BindMount {
    pub host_path: PathBuf,
    pub container_path: PathBuf,
    pub readonly: bool,
}

/// High-level startup parameters; gateway uses this to build a ContainerConfig.
#[derive(Debug, Clone)]
pub struct ContainerStartupConfig {
    pub runtime: ContainerRuntime,
    pub image: String,
    pub container_name: String,
    pub vault_path: PathBuf,
    pub upstream_secret_dir: PathBuf,
    pub host_port: u16,
    pub container_port: u16,
    /// When set, bind-mount this host directory into the container and run
    /// from source instead of the code baked into the image.
    pub dev_mount_source: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum TunnelConfig {
    CloudflareQuick {
        local_port: u16,
    },
    CloudflareNamed {
        tunnel_name: String,
        domain: String,
        config_file: PathBuf,
        local_port: u16,
    },
}
