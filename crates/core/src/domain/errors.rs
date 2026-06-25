use thiserror::Error;

use crate::domain::model::ManagedContainerInfo;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("misdirected_request: {0}")]
    MisdirectedRequest(String),
    #[error("bad_gateway: {0}")]
    BadGateway(String),
}

#[derive(Debug, Error)]
pub enum TokenStoreError {
    #[error("token store unavailable: {0}")]
    Unavailable(String),
}

#[derive(Debug, Error)]
pub enum ContainerError {
    #[error("container runtime not found: {0}")]
    RuntimeNotFound(String),
    #[error("image not found: {0}")]
    ImageNotFound(String),
    #[error("container conflict: {0}")]
    Conflict(String),
    #[error("command failed (exit {code}): {stderr}")]
    CommandFailed { code: i32, stderr: String },
    #[error("command could not be spawned: {0}")]
    SpawnFailed(String),
    #[error("{summary}")]
    StartupFailed {
        summary: String,
        logs: Option<String>,
        started_container: bool,
    },
    #[error("{summary}")]
    OrphanedManagedContainers {
        summary: String,
        installation_id: String,
        containers: Vec<ManagedContainerInfo>,
    },
    #[error("container error: {0}")]
    Other(String),
}

impl ContainerError {
    pub fn summary(&self) -> String {
        match self {
            Self::StartupFailed { summary, .. } => summary.clone(),
            Self::OrphanedManagedContainers { summary, .. } => summary.clone(),
            other => other.to_string(),
        }
    }

    pub fn recent_logs(&self) -> Option<&str> {
        match self {
            Self::StartupFailed {
                logs: Some(logs), ..
            } => Some(logs.as_str()),
            _ => None,
        }
    }

    pub fn started_container(&self) -> bool {
        match self {
            Self::StartupFailed {
                started_container, ..
            } => *started_container,
            _ => false,
        }
    }

    pub fn orphaned_managed_containers(&self) -> Option<&[ManagedContainerInfo]> {
        match self {
            Self::OrphanedManagedContainers { containers, .. } => Some(containers.as_slice()),
            _ => None,
        }
    }

    pub fn requires_explicit_gc(&self) -> bool {
        matches!(self, Self::OrphanedManagedContainers { .. })
    }

    pub fn orphaned_managed_containers_for_installation(
        installation_id: String,
        containers: Vec<ManagedContainerInfo>,
    ) -> Self {
        let details = containers
            .iter()
            .map(|container| format!("{} ({})", container.name, container.state))
            .collect::<Vec<_>>()
            .join(", ");
        let summary = format!(
            "Brain3 found managed MCP containers from this installation already present: {details}"
        );

        Self::OrphanedManagedContainers {
            summary,
            installation_id,
            containers,
        }
    }
}

#[derive(Debug, Error)]
pub enum TunnelError {
    #[error("cloudflared not found — see https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/ for install instructions")]
    CloudflaredNotFound,
    #[error("cloudflared is not logged in — run `cloudflared tunnel login` first")]
    CloudflaredNotLoggedIn,
    #[error("tunnel config not found at {0} — run the setup wizard first")]
    ConfigNotFound(String),
    #[error("tunnel credentials file not found: {0}")]
    CredentialsNotFound(String),
    #[error("tunnel setup failed: {0}")]
    SetupFailed(String),
    #[error("could not spawn cloudflared: {0}")]
    SpawnFailed(String),
    #[error("tunnel '{0}' not found in Cloudflare registry — it may have been deleted (run `cloudflared tunnel list` to check)")]
    TunnelNotFound(String),
    #[error("tunnel not reachable at startup: {0}")]
    NotReachable(String),
    #[error(
        "cloudflare tunnel config port mismatch: config routes to port {config_port}, \
         but gateway is on port {gateway_port} — update the 'service' URL in {config_file}"
    )]
    PortMismatch {
        config_port: u16,
        gateway_port: u16,
        config_file: String,
    },
    #[error("tunnel error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing config: {0}")]
    Missing(String),
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("config conflict: {0}")]
    Conflict(String),
}

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("invalid setup: {0}")]
    Invalid(String),
    #[error("setup I/O error: {0}")]
    Io(String),
    #[error("setup command could not be spawned: {0}")]
    SpawnFailed(String),
    #[error("setup command failed: {command} (exit {code:?}): {stderr}")]
    CommandFailed {
        command: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("unsupported setup operation: {0}")]
    Unsupported(String),
}
