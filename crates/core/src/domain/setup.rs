use std::path::PathBuf;

use crate::domain::model::ContainerRuntime;

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
    pub access_token: String,
    pub username: String,
    pub password: String,
    pub tunnel_mode: TunnelModeDraft,
    pub container_runtime: ContainerRuntime,
    pub vault_path: PathBuf,
    pub container_image: String,
    pub container_host_port: u16,
    pub container_mcp_port: u16,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyStatus {
    pub operating_system: SetupOperatingSystem,
    pub package_manager: Option<PackageManager>,
    pub cloudflared_installed: bool,
    pub docker_installed: bool,
    pub macos_container_installed: Option<bool>,
    pub homebrew_installed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallAction {
    InstallCloudflared,
    InstallDocker,
    InstallMacOSContainer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    Welcome,
    DependencyDoctor,
    VaultPath,
    Auth,
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
pub struct ConnectionCard {
    pub server_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLaunchPlan {
    pub paths: SetupPaths,
    pub env_file: PathBuf,
    pub log_file: PathBuf,
}
