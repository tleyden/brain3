use thiserror::Error;

#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("unsupported_response_type")]
    UnsupportedResponseType,
    #[error("invalid_client")]
    InvalidClient,
    #[error("invalid_request: {0}")]
    InvalidRequest(String),
    #[error("invalid_grant: {0}")]
    InvalidGrant(String),
    #[error("server_error: {0}")]
    ServerError(String),
    #[error("unsupported_grant_type")]
    UnsupportedGrantType,
}

impl OAuthError {
    pub fn error_code(&self) -> &str {
        match self {
            Self::UnsupportedResponseType => "unsupported_response_type",
            Self::InvalidClient => "invalid_client",
            Self::InvalidRequest(_) => "invalid_request",
            Self::InvalidGrant(_) => "invalid_grant",
            Self::ServerError(_) => "server_error",
            Self::UnsupportedGrantType => "unsupported_grant_type",
        }
    }

    pub fn error_description(&self) -> Option<&str> {
        match self {
            Self::InvalidRequest(desc) | Self::InvalidGrant(desc) | Self::ServerError(desc) => {
                Some(desc)
            }
            _ => None,
        }
    }

    pub fn status_code(&self) -> u16 {
        match self {
            Self::UnsupportedResponseType => 400,
            Self::InvalidClient => 401,
            Self::InvalidRequest(_) => 400,
            Self::InvalidGrant(_) => 400,
            Self::ServerError(_) => 500,
            Self::UnsupportedGrantType => 400,
        }
    }
}

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
    #[error("command failed (exit {code}): {stderr}")]
    CommandFailed { code: i32, stderr: String },
    #[error("command could not be spawned: {0}")]
    SpawnFailed(String),
    #[error("{summary}")]
    StartupFailed {
        summary: String,
        logs: Option<String>,
    },
    #[error("container error: {0}")]
    Other(String),
}

impl ContainerError {
    pub fn summary(&self) -> String {
        match self {
            Self::StartupFailed { summary, .. } => summary.clone(),
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
    #[error("tunnel '{name}' is already running ({connections} active connection(s)) — stop the existing cloudflared process first")]
    AlreadyRunning { name: String, connections: usize },
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
