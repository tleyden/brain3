use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use brain3_core::ports::config::ConfigPort;
use brain3_platform::auth_code_store::in_memory::InMemoryAuthCodeStore;
use brain3_platform::config::env_file::EnvFileConfigAdapter;
use brain3_platform::http::router::build_router;
use brain3_platform::http::state::AppState;
use brain3_platform::mcp_proxy::reqwest_proxy::ReqwestMcpProxy;

#[derive(Parser)]
#[command(name = "brain3-gateway", about = "OAuth2 gateway for MCP servers")]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long)]
    env_file: Option<PathBuf>,
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");
    tracing::info!("Received shutdown signal, draining connections...");
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .init();

    let args = Args::parse();

    let config_adapter = EnvFileConfigAdapter::new(args.env_file);
    let config = Arc::new(config_adapter.load().context("failed to load configuration")?);

    let upstream_secret = brain3_platform::config::upstream_secret::read_or_create(
        &config.mcp_reverse_proxy.upstream_secret_file,
    )?;

    let auth_code_store = Arc::new(InMemoryAuthCodeStore::new());
    let mcp_proxy = Arc::new(ReqwestMcpProxy::new());

    let oauth_config = Arc::new(config.oauth.clone());

    let authorize = Arc::new(
        brain3_core::application::authorize::AuthorizeUseCase::new(
            Arc::clone(&oauth_config),
            Arc::clone(&auth_code_store),
        ),
    );
    let token_exchange = Arc::new(
        brain3_core::application::token_exchange::TokenExchangeUseCase::new(
            Arc::clone(&oauth_config),
            Arc::clone(&auth_code_store),
        ),
    );
    let proxy_mcp = Arc::new(
        brain3_core::application::proxy_mcp::ProxyMcpUseCase::new(
            mcp_proxy,
            config.mcp_reverse_proxy.mcp_upstream_url.clone(),
            upstream_secret,
            config.oauth.access_token.clone(),
            config.hostname_validation.clone(),
        ),
    );

    let app_state = AppState {
        authorize,
        token_exchange,
        proxy_mcp,
        config: Arc::clone(&config),
    };

    let router = build_router(app_state);

    let addr = format!("{}:{}", args.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))?;

    tracing::info!("Starting OAuth2 gateway on {}", addr);
    tracing::info!(
        "Proxying MCP traffic to {}",
        config.mcp_reverse_proxy.mcp_upstream_url
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}
