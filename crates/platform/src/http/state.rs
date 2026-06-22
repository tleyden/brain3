use std::sync::Arc;

use oxide_auth::primitives::authorizer::AuthMap;
use oxide_auth::primitives::generator::RandomGenerator;
use oxide_auth::primitives::issuer::TokenMap;
use oxide_auth::primitives::registrar::ClientMap;
use tokio::sync::Mutex;

use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::domain::model::GatewayConfig;
use brain3_core::ports::mcp_proxy::McpProxyPort;

use super::rate_limit::OAuthRateLimiter;
use super::registrar::BrainRegistrar;

pub struct AppState<P: McpProxyPort> {
    /// Used in /oauth/authorize — validates client_id, accepts any redirect_uri.
    pub auth_registrar: Arc<BrainRegistrar>,
    /// Used in /oauth/token — validates client_id + client_secret via ClientMap::check().
    pub token_registrar: Arc<ClientMap>,
    pub authorizer: Arc<Mutex<AuthMap<RandomGenerator>>>,
    pub issuer: Arc<Mutex<TokenMap<RandomGenerator>>>,
    pub proxy_mcp: Arc<ProxyMcpUseCase<P>>,
    pub config: Arc<GatewayConfig>,
    pub rate_limiter: Arc<OAuthRateLimiter>,
}

impl<P: McpProxyPort> Clone for AppState<P> {
    fn clone(&self) -> Self {
        Self {
            auth_registrar: Arc::clone(&self.auth_registrar),
            token_registrar: Arc::clone(&self.token_registrar),
            authorizer: Arc::clone(&self.authorizer),
            issuer: Arc::clone(&self.issuer),
            proxy_mcp: Arc::clone(&self.proxy_mcp),
            config: Arc::clone(&self.config),
            rate_limiter: Arc::clone(&self.rate_limiter),
        }
    }
}
