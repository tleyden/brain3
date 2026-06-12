use std::sync::Arc;

use brain3_core::application::authorize::AuthorizeUseCase;
use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::application::token_exchange::TokenExchangeUseCase;
use brain3_core::domain::model::GatewayConfig;
use brain3_core::ports::auth_code_store::AuthCodeStore;
use brain3_core::ports::mcp_proxy::McpProxyPort;

use super::rate_limit::OAuthRateLimiter;

pub struct AppState<S: AuthCodeStore, P: McpProxyPort> {
    pub authorize: Arc<AuthorizeUseCase<S>>,
    pub token_exchange: Arc<TokenExchangeUseCase<S>>,
    pub proxy_mcp: Arc<ProxyMcpUseCase<P>>,
    pub config: Arc<GatewayConfig>,
    pub rate_limiter: Arc<OAuthRateLimiter>,
}

impl<S: AuthCodeStore, P: McpProxyPort> Clone for AppState<S, P> {
    fn clone(&self) -> Self {
        Self {
            authorize: Arc::clone(&self.authorize),
            token_exchange: Arc::clone(&self.token_exchange),
            proxy_mcp: Arc::clone(&self.proxy_mcp),
            config: Arc::clone(&self.config),
            rate_limiter: Arc::clone(&self.rate_limiter),
        }
    }
}
