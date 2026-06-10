use std::sync::Arc;

use crate::domain::errors::OAuthError;
use crate::domain::model::OAuthConfig;
use crate::domain::oauth::{
    constant_time_eq, verify_pkce, TokenRequest, TokenResponse, ACCESS_TOKEN_LIFETIME_SECS,
};
use crate::domain::redact::elide_secret;
use crate::ports::auth_code_store::AuthCodeStore;

pub struct TokenExchangeUseCase<S: AuthCodeStore> {
    config: Arc<OAuthConfig>,
    store: Arc<S>,
}

impl<S: AuthCodeStore> TokenExchangeUseCase<S> {
    pub fn new(config: Arc<OAuthConfig>, store: Arc<S>) -> Self {
        Self { config, store }
    }

    pub async fn exchange(&self, req: &TokenRequest) -> Result<TokenResponse, OAuthError> {
        tracing::info!(
            grant_type = %req.grant_type,
            client_id = %req.client_id,
            client_secret_hint = %elide_secret(&req.client_secret),
            redirect_uri = ?req.redirect_uri,
            has_code_verifier = req.code_verifier.is_some(),
            "token exchange request received"
        );

        if req.grant_type != "authorization_code" {
            return Err(OAuthError::UnsupportedGrantType);
        }

        self.store.cleanup_expired().await;

        if req.client_id != self.config.client_id {
            tracing::warn!(
                received = %req.client_id,
                expected = %self.config.client_id,
                "token exchange rejected: client_id mismatch"
            );
            return Err(OAuthError::InvalidClient);
        }
        if self.config.client_secret.is_empty() {
            tracing::warn!("token exchange rejected: client_secret not configured on server");
            return Err(OAuthError::ServerError(
                "client secret not configured".into(),
            ));
        }
        if !constant_time_eq(
            req.client_secret.as_bytes(),
            self.config.client_secret.as_bytes(),
        ) {
            tracing::warn!(
                received = %elide_secret(&req.client_secret),
                expected = %elide_secret(&self.config.client_secret),
                "token exchange rejected: client_secret mismatch"
            );
            return Err(OAuthError::InvalidClient);
        }

        let code_data = self
            .store
            .take(&req.code)
            .await
            .ok_or_else(|| {
                tracing::warn!(code = %req.code, "token exchange rejected: auth code not found or expired");
                OAuthError::InvalidGrant("Invalid or expired code".into())
            })?;

        if !constant_time_eq(req.client_id.as_bytes(), code_data.client_id.as_bytes()) {
            tracing::warn!(
                token_client_id = %req.client_id,
                code_client_id = %code_data.client_id,
                "token exchange rejected: client_id mismatch between token request and stored code"
            );
            return Err(OAuthError::InvalidGrant("client_id mismatch".into()));
        }

        if let Some(ref redirect_uri) = req.redirect_uri {
            if !redirect_uri.is_empty()
                && !code_data.redirect_uri.is_empty()
                && redirect_uri != &code_data.redirect_uri
            {
                tracing::warn!(
                    received = %redirect_uri,
                    expected = %code_data.redirect_uri,
                    "token exchange rejected: redirect_uri mismatch"
                );
                return Err(OAuthError::InvalidGrant("redirect_uri mismatch".into()));
            }
        }

        if code_data.pkce_required {
            let challenge = code_data.code_challenge.as_deref().unwrap_or("");
            if challenge.is_empty() {
                tracing::warn!(
                    "token exchange rejected: pkce required but no code_challenge in stored code"
                );
                return Err(OAuthError::InvalidGrant("code_challenge required".into()));
            }
            if let Some(method) = &code_data.code_challenge_method {
                if method != "S256" {
                    tracing::warn!(method = %method, "token exchange rejected: unsupported code_challenge_method");
                    return Err(OAuthError::InvalidGrant(
                        "code_challenge_method must be S256".into(),
                    ));
                }
            }
        }

        if let Some(ref challenge) = code_data.code_challenge {
            if !challenge.is_empty() {
                let verifier = req.code_verifier.as_deref().unwrap_or("");
                if verifier.is_empty() {
                    tracing::warn!(
                        "token exchange rejected: code_challenge present but code_verifier missing"
                    );
                    return Err(OAuthError::InvalidGrant("code_verifier required".into()));
                }
                if !verify_pkce(verifier, challenge) {
                    tracing::warn!("token exchange rejected: PKCE verification failed");
                    return Err(OAuthError::InvalidGrant("PKCE verification failed".into()));
                }
            }
        }

        tracing::info!(
            client_id = %req.client_id,
            "token exchange succeeded: access token issued"
        );

        Ok(TokenResponse {
            access_token: self.config.access_token.clone(),
            token_type: "bearer".into(),
            expires_in: ACCESS_TOKEN_LIFETIME_SECS,
        })
    }
}
