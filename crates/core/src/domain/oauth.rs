use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::errors::OAuthError;

pub const AUTH_CODE_LIFETIME: Duration = Duration::from_secs(300);
pub const ACCESS_TOKEN_LIFETIME_SECS: u64 = 86400;

#[derive(Debug, Clone)]
pub struct AuthCodeData {
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub pkce_required: bool,
    pub expires_at: Instant,
}

#[derive(Debug, Clone)]
pub struct AuthorizeRequest {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TokenRequest {
    pub grant_type: String,
    pub client_id: String,
    pub client_secret: String,
    pub code: String,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

pub fn validate_authorize_request(
    req: &AuthorizeRequest,
    expected_client_id: &str,
    pkce_required: bool,
) -> Result<(), OAuthError> {
    if req.response_type != "code" {
        return Err(OAuthError::UnsupportedResponseType);
    }
    if req.client_id != expected_client_id {
        return Err(OAuthError::InvalidClient);
    }
    if req.redirect_uri.is_empty() {
        return Err(OAuthError::InvalidRequest("redirect_uri required".into()));
    }
    if pkce_required {
        let challenge_empty = req
            .code_challenge
            .as_ref()
            .is_none_or(|s| s.is_empty());
        if challenge_empty {
            return Err(OAuthError::InvalidRequest(
                "code_challenge required".into(),
            ));
        }
        if let Some(method) = &req.code_challenge_method {
            if method != "S256" {
                return Err(OAuthError::InvalidRequest(
                    "code_challenge_method must be S256".into(),
                ));
            }
        }
    }
    Ok(())
}

pub fn verify_pkce(code_verifier: &str, code_challenge: &str) -> bool {
    let digest = Sha256::digest(code_verifier.as_bytes());
    let computed = URL_SAFE_NO_PAD.encode(digest);
    constant_time_eq(computed.as_bytes(), code_challenge.as_bytes())
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

pub fn generate_secure_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
