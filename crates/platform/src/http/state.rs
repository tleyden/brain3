use std::sync::Arc;

use oxide_auth::primitives::authorizer::AuthMap;
use oxide_auth::primitives::generator::RandomGenerator;
use tokio::sync::Mutex;

use crate::token_store::sqlite::SqliteTokenStore;
use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::domain::model::GatewayConfig;
use brain3_core::ports::mcp_proxy::McpProxyPort;

use super::rate_limit::OAuthRateLimiter;
use super::registrar::GatewayRegistrar;

pub struct AppState<P: McpProxyPort> {
    pub registrar: Arc<GatewayRegistrar>,
    pub authorizer: Arc<Mutex<AuthMap<RandomGenerator>>>,
    pub issuer: Arc<Mutex<SqliteTokenStore>>,
    pub proxy_mcp: Arc<ProxyMcpUseCase<P>>,
    pub config: Arc<GatewayConfig>,
    pub rate_limiter: Arc<OAuthRateLimiter>,
}

impl<P: McpProxyPort> Clone for AppState<P> {
    fn clone(&self) -> Self {
        Self {
            registrar: Arc::clone(&self.registrar),
            authorizer: Arc::clone(&self.authorizer),
            issuer: Arc::clone(&self.issuer),
            proxy_mcp: Arc::clone(&self.proxy_mcp),
            config: Arc::clone(&self.config),
            rate_limiter: Arc::clone(&self.rate_limiter),
        }
    }
}
