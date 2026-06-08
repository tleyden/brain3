use std::sync::Arc;
use std::time::Instant;

use crate::domain::errors::OAuthError;
use crate::domain::model::OAuthConfig;
use crate::domain::oauth::{
    constant_time_eq, generate_secure_token, validate_authorize_request, AuthCodeData,
    AuthorizeRequest, AUTH_CODE_LIFETIME,
};
use crate::ports::auth_code_store::AuthCodeStore;

pub struct AuthorizeUseCase<S: AuthCodeStore> {
    config: Arc<OAuthConfig>,
    store: Arc<S>,
}

impl<S: AuthCodeStore> AuthorizeUseCase<S> {
    pub fn new(config: Arc<OAuthConfig>, store: Arc<S>) -> Self {
        Self { config, store }
    }

    pub fn validate(&self, req: &AuthorizeRequest) -> Result<(), OAuthError> {
        validate_authorize_request(req, &self.config.client_id, self.config.pkce_required)
    }

    pub fn login_configured(&self) -> bool {
        !self.config.username.is_empty() && !self.config.password.is_empty()
    }

    pub fn check_credentials(&self, username: &str, password: &str) -> bool {
        constant_time_eq(username.as_bytes(), self.config.username.as_bytes())
            && constant_time_eq(password.as_bytes(), self.config.password.as_bytes())
    }

    pub async fn issue_code(&self, req: &AuthorizeRequest) -> String {
        self.store.cleanup_expired().await;
        let code = generate_secure_token();
        let data = AuthCodeData {
            client_id: req.client_id.clone(),
            redirect_uri: req.redirect_uri.clone(),
            code_challenge: req.code_challenge.clone(),
            code_challenge_method: req.code_challenge_method.clone(),
            pkce_required: self.config.pkce_required,
            expires_at: Instant::now() + AUTH_CODE_LIFETIME,
        };
        self.store.store(code.clone(), data).await;
        code
    }
}
