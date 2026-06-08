use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub port: u16,
    pub host: String,
    pub oauth: OAuthConfig,
    pub mcp_reverse_proxy: MCPReverseProxyConfig,
    pub hostname_validation: HostnameValidationConfig,
}

#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub access_token: String,
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

#[derive(Debug, Clone)]
pub enum ContainerRuntime {
    Docker,
    MacOSContainer,
}

#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub runtime: ContainerRuntime,
    pub image: String,
    pub bind_mounts: Vec<BindMount>,
}

#[derive(Debug, Clone)]
pub struct BindMount {
    pub host_path: PathBuf,
    pub container_path: PathBuf,
    pub readonly: bool,
}

#[derive(Debug, Clone)]
pub enum TunnelProvider {
    Cloudflare,
}

#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub provider: TunnelProvider,
}
