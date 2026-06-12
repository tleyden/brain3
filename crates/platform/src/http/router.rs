use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

use brain3_core::ports::auth_code_store::AuthCodeStore;
use brain3_core::ports::mcp_proxy::McpProxyPort;

use super::assets::{login_logo, login_stylesheet};
use super::health::health;
use super::mcp_handlers::{mcp_reverse_proxy, protected_resource_metadata};
use super::oauth_handlers::{
    oauth_authorize_get, oauth_authorize_post, oauth_metadata, oauth_token,
};
use super::state::AppState;

async fn fallback(req: Request) -> impl IntoResponse {
    tracing::warn!(
        method = %req.method(),
        path = %req.uri(),
        host = ?req.headers().get("host").map(|v| v.to_str().unwrap_or("<invalid>")),
        "no matching route — returning 404"
    );
    StatusCode::NOT_FOUND
}

pub fn build_router<S: AuthCodeStore + 'static, P: McpProxyPort + 'static>(
    state: AppState<S, P>,
) -> Router {
    Router::new()
        .route("/health", get(health))
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_metadata::<S, P>),
        )
        .route(
            "/oauth/authorize",
            get(oauth_authorize_get::<S, P>).post(oauth_authorize_post::<S, P>),
        )
        .route("/oauth/login.css", get(login_stylesheet))
        .route("/oauth/brain3-lockup-light.svg", get(login_logo))
        .route("/oauth/token", post(oauth_token::<S, P>))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource_metadata::<S, P>),
        )
        .route(
            "/mcp",
            get(mcp_reverse_proxy::<S, P>)
                .post(mcp_reverse_proxy::<S, P>)
                .delete(mcp_reverse_proxy::<S, P>),
        )
        .route(
            "/mcp/",
            get(mcp_reverse_proxy::<S, P>)
                .post(mcp_reverse_proxy::<S, P>)
                .delete(mcp_reverse_proxy::<S, P>),
        )
        .route(
            "/mcp/{*path}",
            get(mcp_reverse_proxy::<S, P>)
                .post(mcp_reverse_proxy::<S, P>)
                .delete(mcp_reverse_proxy::<S, P>),
        )
        .fallback(fallback)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
