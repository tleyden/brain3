use std::path::PathBuf;

use crate::domain::model::ContainerRuntime;

pub const DEFAULT_GATEWAY_PORT: u16 = 8421;
pub const DEFAULT_CLIENT_ID: &str = "brain3-oauth2-client";
pub const DEFAULT_USERNAME: &str = "admin";
pub const DEFAULT_CONTAINER_HOST_PORT: u16 = 8420;
pub const DEFAULT_CONTAINER_MCP_PORT: u16 = 8420;
pub const DEFAULT_ACCESS_TOKEN_LIFETIME_SECS: u64 = 3600;
pub const DEFAULT_REFRESH_TOKEN_LIFETIME_SECS: u64 = 90 * 24 * 60 * 60;
pub const DEFAULT_GENERATED_SECRET_BYTES: usize = 32;
pub const DEFAULT_GENERATED_PASSWORD_LENGTH: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupDefaults {
    pub default_container_image: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupPaths {
    pub app_home: PathBuf,
    pub env_file: PathBuf,
    pub cloudflared_dir: PathBuf,
}

impl SetupPaths {
    pub fn new(app_home: PathBuf, env_file: PathBuf, cloudflared_dir: PathBuf) -> Self {
        Self {
            app_home,
            env_file,
            cloudflared_dir,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelModeDraft {
    CloudflareQuick,
    CloudflareNamed { tunnel_name: String, domain: String },
    DirectPublicOrigin { hostname: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupDraftConfig {
    pub gateway_port: u16,
    pub client_id: String,
    pub client_secret: String,
    pub access_token_lifetime_secs: u64,
    pub refresh_token_lifetime_secs: u64,
    pub username: String,
    pub password: String,
    pub tunnel_mode: TunnelModeDraft,
    pub container_runtime: ContainerRuntime,
    pub vault_path: PathBuf,
    pub container_image: String,
    pub container_host_port: u16,
    pub container_mcp_port: u16,
    pub container_network_isolated: bool,
    pub pkce_required: bool,
    pub enforce_hostname_check: bool,
    pub direct_public_origin_hostname: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupOperatingSystem {
    MacOS,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Homebrew,
    Apt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallAction {
    InstallCloudflared,
    InstallDocker,
    InstallMacOSContainer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyAvailability {
    Installed,
    InstallAvailable(InstallAction),
    ManualInstallRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyStatus {
    pub operating_system: SetupOperatingSystem,
    pub package_manager: Option<PackageManager>,
    pub cloudflared: DependencyAvailability,
    pub preferred_container_runtime: DependencyAvailability,
    pub docker_installed: bool,
    pub macos_container_installed: Option<bool>,
    pub homebrew_installed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    Welcome,
    DependencyDoctor,
    VaultPath,
    Auth,
    PortsAndSettings,
    Summary,
    ConnectionCard,
    RuntimeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupSummary {
    pub paths: SetupPaths,
    pub draft: SetupDraftConfig,
    pub dependencies: DependencyStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupPreparation {
    pub paths: SetupPaths,
    pub draft: SetupDraftConfig,
    pub dependencies: DependencyStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizeSetupRequest {
    pub draft: SetupDraftConfig,
    pub generate_password: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionCard {
    pub server_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub username: String,
    pub password: String,
    pub log_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLaunchPlan {
    pub paths: SetupPaths,
    pub env_file: PathBuf,
    pub log_file: PathBuf,
}
