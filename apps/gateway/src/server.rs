use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use brain3_core::application::authorize::AuthorizeUseCase;
use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::application::token_exchange::TokenExchangeUseCase;
use brain3_core::domain::model::GatewayConfig;
use brain3_core::domain::setup::RuntimeLaunchPlan;
use brain3_core::ports::config::ConfigPort;
use brain3_platform::auth_code_store::in_memory::InMemoryAuthCodeStore;
use brain3_platform::config::env_file::EnvFileConfigAdapter;
use brain3_platform::http::rate_limit::OAuthRateLimiter;
use brain3_platform::http::router::build_router;
use brain3_platform::http::state::AppState;
use brain3_platform::mcp_proxy::reqwest_proxy::ReqwestMcpProxy;
use brain3_platform::runtime::{bootstrap_configured_runtime, RuntimeBootstrap};
use brain3_platform::token_store::sqlite::SqliteTokenStore;

use crate::{apply_runtime_overrides, RuntimeOverrides};

pub struct ConfiguredGatewaySession {
    pub runtime: RuntimeBootstrap,
    pub server: Option<GatewayServerHandle>,
    pub display_url: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayServerStatus {
    NotStarted,
    Running {
        bind_addr: String,
        local_url: String,
    },
    Stopped {
        bind_addr: String,
    },
}

#[allow(dead_code)]
pub struct GatewayServerHandle {
    bind_addr: String,
    local_url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<Result<()>>,
}

#[allow(dead_code)]
impl GatewayServerHandle {
    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    pub fn local_url(&self) -> &str {
        &self.local_url
    }

    pub fn status(&self) -> GatewayServerStatus {
        if self.join_handle.is_finished() {
            GatewayServerStatus::Stopped {
                bind_addr: self.bind_addr.clone(),
            }
        } else {
            GatewayServerStatus::Running {
                bind_addr: self.bind_addr.clone(),
                local_url: self.local_url.clone(),
            }
        }
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        self.join_handle
            .await
            .context("gateway server task join failed")?
    }
}

pub async fn run_gateway_server_until<F>(
    host: &str,
    config: Arc<GatewayConfig>,
    upstream_secret: String,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let listener = bind_listener(host, config.port).await?;
    let bind_addr = listener
        .local_addr()
        .context("failed to resolve bound gateway address")?;
    let local_url = local_url_from_addr(bind_addr);
    let router = build_gateway_router(Arc::clone(&config), upstream_secret)?;

    tracing::info!(bind_addr = %bind_addr, local_url = %local_url, "starting OAuth2 gateway");
    tracing::info!(
        "Proxying MCP traffic to {}",
        config.mcp_reverse_proxy.mcp_upstream_url
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .context("server error")
}

#[allow(dead_code)]
pub async fn spawn_gateway_server(
    host: &str,
    config: Arc<GatewayConfig>,
    upstream_secret: String,
) -> Result<GatewayServerHandle> {
    let listener = bind_listener(host, config.port).await?;
    let bind_addr = listener
        .local_addr()
        .context("failed to resolve bound gateway address")?;
    let bind_addr_display = bind_addr.to_string();
    let local_url = local_url_from_addr(bind_addr);
    let router = build_gateway_router(config, upstream_secret)?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    tracing::info!(
        bind_addr = %bind_addr_display,
        local_url = %local_url,
        "starting OAuth2 gateway in background"
    );

    let join_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
            .context("server error")
    });

    Ok(GatewayServerHandle {
        bind_addr: bind_addr_display,
        local_url,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    })
}

pub async fn spawn_configured_gateway_session(
    host: &str,
    launch_plan: RuntimeLaunchPlan,
    runtime_overrides: RuntimeOverrides,
) -> Result<ConfiguredGatewaySession> {
    let mut config = EnvFileConfigAdapter::new(Some(launch_plan.env_file.clone()))
        .load()
        .context("failed to load configuration")?;
    apply_runtime_overrides(&mut config, &runtime_overrides)?;
    let config = Arc::new(config);
    let runtime = bootstrap_configured_runtime(Arc::clone(&config), launch_plan).await?;

    let (server, display_url) = if runtime.can_start_gateway() {
        let server = spawn_gateway_server(
            host,
            Arc::clone(&runtime.config),
            runtime.upstream_secret.clone(),
        )
        .await?;
        let local_url = server.local_url().to_string();
        let display_url = runtime.display_url(&local_url);
        tracing::debug!(
            tunnel_public_url = ?runtime.public_url,
            local_url = %local_url,
            display_url = %display_url,
            "resolved display URL for connection card (tunnel URL wins over local if present)"
        );
        (Some(server), Some(display_url))
    } else {
        (None, None)
    };

    Ok(ConfiguredGatewaySession {
        runtime,
        server,
        display_url,
    })
}

async fn bind_listener(host: &str, port: u16) -> Result<TcpListener> {
    let addr = format!("{host}:{port}");
    TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))
}

fn build_gateway_router(config: Arc<GatewayConfig>, upstream_secret: String) -> Result<Router> {
    let auth_code_store = Arc::new(InMemoryAuthCodeStore::new());
    let mcp_proxy = Arc::new(ReqwestMcpProxy::new());
    let oauth_config = Arc::new(config.oauth.clone());
    let token_store: Arc<dyn brain3_core::ports::token_store::TokenStore> = Arc::new(
        SqliteTokenStore::from_path(&config.token_db_path).with_context(|| {
            format!(
                "failed to initialize token store at {}",
                config.token_db_path.display()
            )
        })?,
    );

    spawn_token_cleanup_task(Arc::clone(&token_store));

    let authorize = Arc::new(AuthorizeUseCase::new(
        Arc::clone(&oauth_config),
        Arc::clone(&auth_code_store),
    ));
    let token_exchange = Arc::new(TokenExchangeUseCase::new(
        Arc::clone(&oauth_config),
        Arc::clone(&auth_code_store),
        Arc::clone(&token_store),
    ));
    let proxy_mcp = Arc::new(ProxyMcpUseCase::new(
        mcp_proxy,
        config.mcp_reverse_proxy.mcp_upstream_url.clone(),
        upstream_secret,
        token_store,
        config.hostname_validation.clone(),
    ));

    let app_state = AppState {
        authorize,
        token_exchange,
        proxy_mcp,
        config,
        rate_limiter: Arc::new(OAuthRateLimiter::new()),
    };

    Ok(build_router(app_state))
}

fn spawn_token_cleanup_task(token_store: Arc<dyn brain3_core::ports::token_store::TokenStore>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            if let Err(error) = token_store.cleanup_expired().await {
                tracing::warn!(%error, "failed to clean up expired access tokens");
            }
        }
    });
}

fn local_url_from_addr(addr: SocketAddr) -> String {
    match addr {
        SocketAddr::V4(v4) => {
            let ip = if v4.ip().is_unspecified() {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            } else {
                IpAddr::V4(*v4.ip())
            };
            format!("http://{}:{}", ip, v4.port())
        }
        SocketAddr::V6(v6) => {
            let ip = if v6.ip().is_unspecified() {
                IpAddr::V6(Ipv6Addr::LOCALHOST)
            } else {
                IpAddr::V6(*v6.ip())
            };
            format!("http://[{}]:{}", ip, v6.port())
        }
    }
}
