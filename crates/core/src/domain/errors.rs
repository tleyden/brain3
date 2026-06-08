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
pub enum ConfigError {
    #[error("missing config: {0}")]
    Missing(String),
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("config conflict: {0}")]
    Conflict(String),
}
