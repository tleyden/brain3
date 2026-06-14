use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::domain::errors::OAuthError;
use crate::domain::model::OAuthConfig;
use crate::domain::oauth::{
    constant_time_eq, generate_secure_token, verify_pkce, TokenRequest, TokenResponse,
};
use crate::domain::redact::elide_secret;
use crate::ports::auth_code_store::AuthCodeStore;
use crate::ports::token_store::{StoredTokenData, StoredTokenKind, TokenStore};

pub struct TokenExchangeUseCase<S: AuthCodeStore> {
    config: Arc<OAuthConfig>,
    store: Arc<S>,
    token_store: Arc<dyn TokenStore>,
}

impl<S: AuthCodeStore> TokenExchangeUseCase<S> {
    pub fn new(config: Arc<OAuthConfig>, store: Arc<S>, token_store: Arc<dyn TokenStore>) -> Self {
        Self {
            config,
            store,
            token_store,
        }
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

        match req.grant_type.as_str() {
            "authorization_code" => self.exchange_authorization_code(req).await,
            "refresh_token" => self.exchange_refresh_token(req).await,
            _ => Err(OAuthError::UnsupportedGrantType),
        }
    }

    fn validate_client(&self, req: &TokenRequest) -> Result<(), OAuthError> {
        if req.client_id != self.config.client_id {
            tracing::warn!(
                received = %req.client_id,
                expected = %self.config.client_id,
                "token exchange rejected: client_id mismatch"
            );
            Err(OAuthError::InvalidClient)
        } else if self.config.client_secret.is_empty() {
            tracing::warn!("token exchange rejected: client_secret not configured on server");
            Err(OAuthError::ServerError(
                "client secret not configured".into(),
            ))
        } else if !constant_time_eq(
            req.client_secret.as_bytes(),
            self.config.client_secret.as_bytes(),
        ) {
            tracing::warn!(
                received = %elide_secret(&req.client_secret),
                expected = %elide_secret(&self.config.client_secret),
                "token exchange rejected: client_secret mismatch"
            );
            Err(OAuthError::InvalidClient)
        } else {
            Ok(())
        }
    }

    async fn exchange_authorization_code(
        &self,
        req: &TokenRequest,
    ) -> Result<TokenResponse, OAuthError> {
        self.store.cleanup_expired().await;
        self.validate_client(req)?;

        let code = req
            .code
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| OAuthError::InvalidRequest("code required".into()))?;

        let code_data = self.store.take(code).await.ok_or_else(|| {
            tracing::warn!(code = %code, "token exchange rejected: auth code not found or expired");
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

        let response = self.issue_token_pair(&req.client_id).await?;

        tracing::info!(
            client_id = %req.client_id,
            "token exchange succeeded: access and refresh tokens issued"
        );

        Ok(response)
    }

    async fn exchange_refresh_token(
        &self,
        req: &TokenRequest,
    ) -> Result<TokenResponse, OAuthError> {
        self.validate_client(req)?;

        let refresh_token = req
            .refresh_token
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| OAuthError::InvalidRequest("refresh_token required".into()))?;

        let stored = self
            .token_store
            .get(refresh_token)
            .await
            .map_err(|error| {
                tracing::error!(%error, "refresh exchange failed to load refresh token");
                OAuthError::ServerError("failed to load refresh token".into())
            })?
            .ok_or_else(|| OAuthError::InvalidGrant("Invalid or expired refresh token".into()))?;

        if stored.kind != StoredTokenKind::Refresh {
            tracing::warn!(
                refresh_token_hint = %elide_secret(refresh_token),
                "refresh exchange rejected: token kind was not refresh"
            );
            return Err(OAuthError::InvalidGrant("Invalid refresh token".into()));
        }

        if stored.expires_at <= SystemTime::now() {
            tracing::warn!(
                refresh_token_hint = %elide_secret(refresh_token),
                client_id = %stored.client_id,
                "refresh exchange rejected: refresh token expired"
            );
            return Err(OAuthError::InvalidGrant(
                "Invalid or expired refresh token".into(),
            ));
        }

        if !constant_time_eq(req.client_id.as_bytes(), stored.client_id.as_bytes()) {
            tracing::warn!(
                request_client_id = %req.client_id,
                stored_client_id = %stored.client_id,
                "refresh exchange rejected: client_id mismatch"
            );
            return Err(OAuthError::InvalidGrant("client_id mismatch".into()));
        }

        let response = self.issue_token_pair(&req.client_id).await?;

        self.token_store
            .revoke(refresh_token)
            .await
            .map_err(|error| {
                tracing::error!(
                    %error,
                    refresh_token_hint = %elide_secret(refresh_token),
                    "refresh exchange failed to revoke used refresh token"
                );
                OAuthError::ServerError("failed to rotate refresh token".into())
            })?;

        tracing::info!(
            client_id = %req.client_id,
            "refresh exchange succeeded: access and refresh tokens rotated"
        );

        Ok(response)
    }

    async fn issue_token_pair(&self, client_id: &str) -> Result<TokenResponse, OAuthError> {
        let access_token = generate_secure_token();
        let refresh_token = generate_secure_token();
        let access_expires_in = self.config.access_token_lifetime_secs;
        let access_expires_at = SystemTime::now() + Duration::from_secs(access_expires_in);
        let refresh_expires_at =
            SystemTime::now() + Duration::from_secs(self.config.refresh_token_lifetime_secs);

        self.token_store
            .store(
                access_token.clone(),
                StoredTokenData {
                    client_id: client_id.to_string(),
                    kind: StoredTokenKind::Access,
                    expires_at: access_expires_at,
                },
            )
            .await
            .map_err(|error| {
                tracing::error!(%error, "token exchange failed to persist access token");
                OAuthError::ServerError("failed to persist access token".into())
            })?;

        match self
            .token_store
            .store(
                refresh_token.clone(),
                StoredTokenData {
                    client_id: client_id.to_string(),
                    kind: StoredTokenKind::Refresh,
                    expires_at: refresh_expires_at,
                },
            )
            .await
        {
            Ok(()) => {}
            Err(error) => {
                if let Err(revoke_error) = self.token_store.revoke(&access_token).await {
                    tracing::error!(
                        %revoke_error,
                        access_token_hint = %elide_secret(&access_token),
                        "token exchange failed to clean up access token after refresh token write failure"
                    );
                }
                tracing::error!(%error, "token exchange failed to persist refresh token");
                return Err(OAuthError::ServerError(
                    "failed to persist refresh token".into(),
                ));
            }
        }

        Ok(TokenResponse {
            access_token,
            token_type: "bearer".into(),
            expires_in: access_expires_in,
            refresh_token: Some(refresh_token),
        })
    }
}
