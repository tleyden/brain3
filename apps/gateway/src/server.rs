use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use oxide_auth::primitives::authorizer::AuthMap;
use oxide_auth::primitives::generator::RandomGenerator;
use tokio::net::TcpListener;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::domain::model::{AccessMode, GatewayConfig};
use brain3_core::domain::setup::{RuntimeLaunchPlan, RuntimeStartupPolicy};
use brain3_core::ports::config::ConfigPort;
use brain3_platform::config::env_file::EnvFileConfigAdapter;
use brain3_platform::http::rate_limit::OAuthRateLimiter;
use brain3_platform::http::registrar::GatewayRegistrar;
use brain3_platform::http::router::{build_local_router, build_router};
use brain3_platform::http::state::AppState;
use brain3_platform::mcp_proxy::reqwest_proxy::ReqwestMcpProxy;
use brain3_platform::runtime::{bootstrap_configured_runtime, RuntimeBootstrap};
use brain3_platform::token_store::sqlite::SqliteTokenStore;

use crate::{apply_runtime_overrides, release, RuntimeOverrides};

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
    shutdown_tx: Option<watch::Sender<bool>>,
    join_handles: Vec<JoinHandle<Result<()>>>,
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
        if self.join_handles.iter().any(JoinHandle::is_finished) {
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
            let _ = tx.send(true);
        }

        for join_handle in self.join_handles {
            join_handle
                .await
                .context("gateway server task join failed")??;
        }

        Ok(())
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
    tracing::info!(
        access_mode = ?config.access_mode,
        "access mode: binding only needed ports"
    );

    let oauth_listener = if config.access_mode != AccessMode::Local {
        Some(bind_listener(host, config.port).await?)
    } else {
        None
    };
    let local_listener = bind_local_mcp_listener(&config).await?;
    let local_mcp_url = local_listener
        .as_ref()
        .and_then(|(listener, _)| listener.local_addr().ok())
        .map(local_url_from_addr);
    let app_state = build_gateway_state(Arc::clone(&config), upstream_secret)?;
    let (shutdown_tx, _) = watch::channel(false);

    tracing::info!(
        "Proxying MCP traffic to {}",
        config.mcp_reverse_proxy.mcp_upstream_url
    );

    if let Some(local_mcp) = config.local_mcp.as_ref() {
        tracing::info!(
            url = %format!("http://localhost:{}/mcp", local_mcp.port),
            "local MCP access enabled"
        );
    }

    if let Some(listener) = oauth_listener {
        let bind_addr = listener
            .local_addr()
            .context("failed to resolve bound gateway address")?;
        let local_url = local_url_from_addr(bind_addr);
        let router = build_router(app_state.clone());
        let local_task = if let Some((listener, local_port)) = local_listener {
            let router = build_local_router(app_state);
            let mut shutdown_rx = shutdown_tx.subscribe();
            tracing::info!(
                bind_addr = %format!("127.0.0.1:{local_port}"),
                local_url = local_mcp_url.as_deref().unwrap_or("http://localhost"),
                "starting local MCP listener"
            );
            Some(tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.changed().await;
                    })
                    .await
                    .context("local MCP server error")
            }))
        } else {
            None
        };

        tracing::info!(
            bind_addr = %bind_addr,
            local_url = %local_url,
            token_db_path = %config.token_db_path.display(),
            brain3_version = release::APP_VERSION,
            oauth_implementation = release::OAUTH_IMPLEMENTATION,
            oxide_auth_version = release::OXIDE_AUTH_VERSION,
            oxide_auth_async_version = release::OXIDE_AUTH_ASYNC_VERSION,
            oxide_auth_axum_version = release::OXIDE_AUTH_AXUM_VERSION,
            oauth_token_store = "sqlite",
            client_id = %config.oauth.client_id,
            pkce_required = config.oauth.pkce_required,
            "starting OAuth2 gateway"
        );

        let shutdown_tx_for_public = shutdown_tx.clone();
        let serve_result = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                shutdown.await;
                let _ = shutdown_tx_for_public.send(true);
            })
            .await
            .context("server error");

        let _ = shutdown_tx.send(true);
        if let Some(local_task) = local_task {
            local_task
                .await
                .context("local MCP server task join failed")??;
        }

        serve_result
    } else if let Some((listener, local_port)) = local_listener {
        let bind_addr = listener
            .local_addr()
            .context("failed to resolve bound local MCP address")?;
        let local_url = local_url_from_addr(bind_addr);
        let router = build_local_router(app_state);

        tracing::info!(
            bind_addr = %format!("127.0.0.1:{local_port}"),
            local_url = %local_url,
            "starting local MCP listener"
        );
        tracing::info!("OAuth gateway port not bound because access mode is local-only");

        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await
            .context("local MCP server error")
    } else {
        bail!("no gateway listeners enabled; check B3_ACCESS_MODE and local MCP configuration")
    }
}

#[allow(dead_code)]
pub async fn spawn_gateway_server(
    host: &str,
    config: Arc<GatewayConfig>,
    upstream_secret: String,
) -> Result<GatewayServerHandle> {
    tracing::info!(
        access_mode = ?config.access_mode,
        "access mode: binding only needed ports"
    );

    let oauth_listener = if config.access_mode != AccessMode::Local {
        Some(bind_listener(host, config.port).await?)
    } else {
        None
    };
    let local_listener = bind_local_mcp_listener(&config).await?;
    let local_mcp_url = local_listener
        .as_ref()
        .and_then(|(listener, _)| listener.local_addr().ok())
        .map(local_url_from_addr);
    let bind_addr = if let Some(listener) = oauth_listener.as_ref() {
        listener
            .local_addr()
            .context("failed to resolve bound gateway address")?
    } else if let Some((listener, _)) = local_listener.as_ref() {
        listener
            .local_addr()
            .context("failed to resolve bound local MCP address")?
    } else {
        bail!("no gateway listeners enabled; check B3_ACCESS_MODE and local MCP configuration");
    };
    let bind_addr_display = bind_addr.to_string();
    let local_url = local_url_from_addr(bind_addr);
    let app_state = build_gateway_state(Arc::clone(&config), upstream_secret)?;
    let (shutdown_tx, _) = watch::channel(false);
    let mut join_handles = Vec::new();

    tracing::info!(
        "Proxying MCP traffic to {}",
        config.mcp_reverse_proxy.mcp_upstream_url
    );

    if let Some(listener) = oauth_listener {
        tracing::info!(
            bind_addr = %bind_addr_display,
            local_url = %local_url,
            token_db_path = %config.token_db_path.display(),
            brain3_version = release::APP_VERSION,
            oauth_implementation = release::OAUTH_IMPLEMENTATION,
            oxide_auth_version = release::OXIDE_AUTH_VERSION,
            oxide_auth_async_version = release::OXIDE_AUTH_ASYNC_VERSION,
            oxide_auth_axum_version = release::OXIDE_AUTH_AXUM_VERSION,
            oauth_token_store = "sqlite",
            client_id = %config.oauth.client_id,
            pkce_required = config.oauth.pkce_required,
            "starting OAuth2 gateway in background"
        );
        let router = build_router(app_state.clone());
        let mut shutdown_rx = shutdown_tx.subscribe();
        join_handles.push(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await
                .context("server error")
        }));
    } else {
        tracing::info!("OAuth gateway port not bound because access mode is local-only");
    }

    if let Some((listener, local_port)) = local_listener {
        tracing::info!(
            bind_addr = %format!("127.0.0.1:{local_port}"),
            local_url = %local_mcp_url
                .unwrap_or_else(|| format!("http://localhost:{local_port}")),
            "starting local MCP listener in background"
        );
        let router = build_local_router(app_state);
        let mut shutdown_rx = shutdown_tx.subscribe();
        join_handles.push(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await
                .context("local MCP server error")
        }));
    }

    Ok(GatewayServerHandle {
        bind_addr: bind_addr_display,
        local_url,
        shutdown_tx: Some(shutdown_tx),
        join_handles,
    })
}

pub async fn spawn_configured_gateway_session(
    host: &str,
    launch_plan: RuntimeLaunchPlan,
    runtime_overrides: RuntimeOverrides,
    startup_policy: RuntimeStartupPolicy,
) -> Result<ConfiguredGatewaySession> {
    let mut config = EnvFileConfigAdapter::with_token_db_home_override(
        Some(launch_plan.env_file.clone()),
        runtime_overrides.brain3_home.clone(),
    )
    .load()
    .context("failed to load configuration")?;
    apply_runtime_overrides(&mut config, &runtime_overrides)?;
    let config = Arc::new(config);
    let runtime =
        bootstrap_configured_runtime(Arc::clone(&config), launch_plan, startup_policy).await?;

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

async fn bind_local_mcp_listener(config: &GatewayConfig) -> Result<Option<(TcpListener, u16)>> {
    if config.access_mode == AccessMode::Remote {
        return Ok(None);
    }

    let Some(local_mcp) = config.local_mcp.as_ref() else {
        return Ok(None);
    };

    let listener = bind_listener("127.0.0.1", local_mcp.port).await?;
    Ok(Some((listener, local_mcp.port)))
}

fn build_gateway_state(
    config: Arc<GatewayConfig>,
    upstream_secret: String,
) -> Result<AppState<ReqwestMcpProxy>> {
    let registrar = Arc::new(GatewayRegistrar::new(
        &config.oauth.client_id,
        config.oauth.client_secret.as_bytes().to_vec(),
    ));

    let authorizer = Arc::new(Mutex::new(AuthMap::new(RandomGenerator::new(32))));
    let issuer = Arc::new(Mutex::new(
        SqliteTokenStore::from_path(
            &config.token_db_path,
            config.oauth.access_token_lifetime_secs,
            config.oauth.refresh_token_lifetime_secs,
        )
        .context("failed to initialize sqlite OAuth issuer")?,
    ));

    let mcp_proxy = Arc::new(ReqwestMcpProxy::new());
    let proxy_mcp = Arc::new(ProxyMcpUseCase::new(
        mcp_proxy,
        config.mcp_reverse_proxy.mcp_upstream_url.clone(),
        upstream_secret,
        config.hostname_validation.clone(),
    ));

    let app_state = AppState {
        registrar,
        authorizer,
        issuer,
        proxy_mcp,
        config,
        rate_limiter: Arc::new(OAuthRateLimiter::new()),
    };

    Ok(app_state)
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
