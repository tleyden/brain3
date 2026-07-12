use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use brain3_core::application::mcp_router::McpRouterUseCase;
use brain3_core::application::native_mcp_tool_registry::NativeMcpToolRegistry;
use oxide_auth::primitives::authorizer::AuthMap;
use oxide_auth::primitives::generator::RandomGenerator;
use tokio::net::TcpListener;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::application::remote_mcp_container_client::RemoteMcpContainerClient;
use brain3_core::domain::model::{AccessMode, GatewayConfig, PluginMcpContainerAuth};
use brain3_core::domain::setup::{RuntimeLaunchPlan, RuntimeStartupPolicy};
use brain3_core::ports::config::ConfigPort;
use brain3_core::ports::native_mcp_tool::NativeMcpTool;
use brain3_platform::config::env_file::EnvFileConfigAdapter;
use brain3_platform::container::startup::StartedPluginMcpContainer;
use brain3_platform::http::rate_limit::OAuthRateLimiter;
use brain3_platform::http::registrar::GatewayRegistrar;
use brain3_platform::http::router::{build_local_router, build_router};
use brain3_platform::http::state::AppState;
use brain3_platform::mcp_proxy::reqwest_proxy::ReqwestMcpProxy;
use brain3_platform::native_mcp_tools::whisper_transcribe::{
    WhisperTranscribeConfig, WhisperTranscribeTool,
};
use brain3_platform::runtime::{bootstrap_configured_runtime, RuntimeBootstrap};
use brain3_platform::token_store::sqlite::SqliteTokenStore;

use crate::{apply_runtime_overrides, release, RuntimeOverrides};

const PLUGIN_MCP_CLIENT_INIT_TIMEOUT: Duration = Duration::from_secs(15);
const PLUGIN_MCP_CLIENT_INIT_RETRY_INTERVAL: Duration = Duration::from_millis(250);

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

#[allow(dead_code)]
pub async fn run_gateway_server_until<F>(
    host: &str,
    config: Arc<GatewayConfig>,
    upstream_secret: String,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    run_gateway_server_until_with_plugin_containers(
        host,
        config,
        upstream_secret,
        Vec::new(),
        shutdown,
    )
    .await
}

pub async fn run_gateway_server_until_with_plugin_containers<F>(
    host: &str,
    config: Arc<GatewayConfig>,
    upstream_secret: String,
    plugin_mcp_containers: Vec<StartedPluginMcpContainer>,
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
    let app_state =
        build_gateway_state(Arc::clone(&config), upstream_secret, &plugin_mcp_containers).await?;
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
    spawn_gateway_server_with_plugin_containers(host, config, upstream_secret, Vec::new()).await
}

pub async fn spawn_gateway_server_with_plugin_containers(
    host: &str,
    config: Arc<GatewayConfig>,
    upstream_secret: String,
    plugin_mcp_containers: Vec<StartedPluginMcpContainer>,
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
    let app_state =
        build_gateway_state(Arc::clone(&config), upstream_secret, &plugin_mcp_containers).await?;
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
        let server = spawn_gateway_server_with_plugin_containers(
            host,
            Arc::clone(&runtime.config),
            runtime.upstream_secret.clone(),
            runtime.plugin_mcp_containers.clone(),
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

async fn build_gateway_state(
    config: Arc<GatewayConfig>,
    upstream_secret: String,
    plugin_mcp_containers: &[StartedPluginMcpContainer],
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
        Arc::clone(&mcp_proxy),
        config.mcp_reverse_proxy.mcp_upstream_url.clone(),
        upstream_secret,
        config.hostname_validation.clone(),
    ));
    let native_tools = Arc::new(NativeMcpToolRegistry::new(native_mcp_tools_from_config(
        config.as_ref(),
    )?));
    let plugin_clients =
        build_plugin_mcp_clients(Arc::clone(&mcp_proxy), plugin_mcp_containers).await;
    let mcp_router = Arc::new(McpRouterUseCase::new_with_plugin_containers(
        proxy_mcp,
        native_tools,
        plugin_clients,
    ));
    mcp_router.log_startup_tool_inventory().await;

    let app_state = AppState {
        registrar,
        authorizer,
        issuer,
        proxy_mcp: mcp_router,
        config,
        rate_limiter: Arc::new(OAuthRateLimiter::new()),
    };

    Ok(app_state)
}

async fn build_plugin_mcp_clients(
    proxy: Arc<ReqwestMcpProxy>,
    containers: &[StartedPluginMcpContainer],
) -> Vec<Arc<RemoteMcpContainerClient<ReqwestMcpProxy>>> {
    let mut clients = Vec::new();
    for container in containers {
        let mcp_url = plugin_mcp_url(container);
        let bearer_token = match plugin_bearer_token(container) {
            Ok(token) => token,
            Err(error) => {
                tracing::error!(
                    container = %container.config.name,
                    error = %error,
                    "skipping Plugin MCP Container client because bearer token could not be loaded"
                );
                continue;
            }
        };

        match initialize_plugin_mcp_client_with_retry(
            container.config.name.clone(),
            mcp_url,
            bearer_token,
            Arc::clone(&proxy),
        )
        .await
        {
            Ok(client) => clients.push(Arc::new(client)),
            Err(error) => {
                tracing::error!(
                    container = %container.config.name,
                    error = %error,
                    "skipping Plugin MCP Container tools because initialize/tools-list failed"
                );
            }
        }
    }

    clients
}

async fn initialize_plugin_mcp_client_with_retry(
    container_name: String,
    mcp_url: String,
    bearer_token: Option<String>,
    proxy: Arc<ReqwestMcpProxy>,
) -> Result<RemoteMcpContainerClient<ReqwestMcpProxy>> {
    let started_at = Instant::now();
    let deadline = started_at + PLUGIN_MCP_CLIENT_INIT_TIMEOUT;
    let mut attempt = 1_u32;

    loop {
        match RemoteMcpContainerClient::initialize_and_cache_tools(
            container_name.clone(),
            mcp_url.clone(),
            bearer_token.clone(),
            Arc::clone(&proxy),
        )
        .await
        {
            Ok(client) => return Ok(client),
            Err(error) if Instant::now() < deadline => {
                tracing::warn!(
                    container = %container_name,
                    attempt,
                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "Plugin MCP Container initialize/tools-list failed; retrying"
                );
                attempt += 1;
                tokio::time::sleep(PLUGIN_MCP_CLIENT_INIT_RETRY_INTERVAL).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn plugin_mcp_url(container: &StartedPluginMcpContainer) -> String {
    match &container.container_ip {
        Some(container_ip) => format!(
            "http://{}:{}/mcp",
            container_ip, container.config.container_port
        ),
        None => format!("http://127.0.0.1:{}/mcp", container.host_port),
    }
}

fn plugin_bearer_token(container: &StartedPluginMcpContainer) -> Result<Option<String>> {
    match &container.config.auth {
        PluginMcpContainerAuth::None => Ok(None),
        PluginMcpContainerAuth::BearerToken { secret_file, .. } => {
            let token = std::fs::read_to_string(secret_file)
                .with_context(|| format!("failed to read {}", secret_file.display()))?;
            Ok(Some(token.trim().to_string()))
        }
    }
}

fn native_mcp_tools_from_config(config: &GatewayConfig) -> Result<Vec<Arc<dyn NativeMcpTool>>> {
    let transcription = &config.native_audio_transcription;
    if !transcription.enabled {
        tracing::debug!("native audio transcription disabled");
        return Ok(Vec::new());
    }

    if !transcription.model_path.exists() {
        tracing::warn!(
            model = %transcription.model,
            model_path = %transcription.model_path.display(),
            "native audio transcription enabled but whisper model file is missing; \
             run `brain3 --setup` to download or reconfigure the model"
        );
        return Ok(Vec::new());
    }

    tracing::info!(
        model = %transcription.model,
        model_path = %transcription.model_path.display(),
        max_audio_bytes = transcription.max_audio_bytes,
        "registering native MCP whisper transcription tool"
    );

    Ok(vec![Arc::new(WhisperTranscribeTool::new(
        WhisperTranscribeConfig {
            model_path: transcription.model_path.clone(),
            max_audio_bytes: transcription.max_audio_bytes,
        },
    ))])
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use brain3_core::domain::model::{
        AccessMode, ContainerRuntime, GatewayConfig, HostnameValidationConfig,
        MCPReverseProxyConfig, NativeAudioTranscriptionConfig, OAuthConfig, PluginMcpContainerAuth,
        PluginMcpContainerConfig,
    };

    use super::*;

    #[test]
    fn native_mcp_tools_skip_enabled_transcription_when_model_file_is_missing() {
        let config = gateway_config_with_native_audio(NativeAudioTranscriptionConfig {
            enabled: true,
            model: "base.en".into(),
            model_path: PathBuf::from("/tmp/brain3-test-missing-whisper-model/ggml-base.en.bin"),
            max_audio_bytes: 50 * 1024 * 1024,
        });

        let tools = native_mcp_tools_from_config(&config).expect("tool loading should succeed");

        assert!(
            tools.is_empty(),
            "missing whisper model file should prevent advertising transcribe_audio_file"
        );
    }

    #[test]
    fn plugin_mcp_url_uses_container_ip_when_available() {
        let container = StartedPluginMcpContainer {
            config: sample_plugin_container_config(8420, PluginMcpContainerAuth::None),
            host_port: 18420,
            container_ip: Some("172.18.0.2".into()),
        };

        assert_eq!(plugin_mcp_url(&container), "http://172.18.0.2:8420/mcp");
    }

    #[test]
    fn plugin_mcp_url_uses_loopback_host_port_without_container_ip() {
        let container = StartedPluginMcpContainer {
            config: sample_plugin_container_config(8420, PluginMcpContainerAuth::None),
            host_port: 18420,
            container_ip: None,
        };

        assert_eq!(plugin_mcp_url(&container), "http://127.0.0.1:18420/mcp");
    }

    #[test]
    fn plugin_bearer_token_reads_and_trims_secret_file() {
        let secret_file = std::env::temp_dir().join(format!(
            "brain3-plugin-token-test-{}-{}.token",
            std::process::id(),
            "phase3"
        ));
        std::fs::write(&secret_file, "secret-token\n").expect("write secret");
        let container = StartedPluginMcpContainer {
            config: sample_plugin_container_config(
                8420,
                PluginMcpContainerAuth::BearerToken {
                    secret_file,
                    secret_mount_path: "/run/secrets/mcp_bearer_token".into(),
                },
            ),
            host_port: 18420,
            container_ip: None,
        };

        assert_eq!(
            plugin_bearer_token(&container).expect("token should load"),
            Some("secret-token".into())
        );
        if let PluginMcpContainerAuth::BearerToken { secret_file, .. } = &container.config.auth {
            let _ = std::fs::remove_file(secret_file);
        }
    }

    fn sample_plugin_container_config(
        container_port: u16,
        auth: PluginMcpContainerAuth,
    ) -> PluginMcpContainerConfig {
        PluginMcpContainerConfig {
            name: "fluensy_learn".into(),
            runtime: ContainerRuntime::Docker,
            image: "ghcr.io/example/fluensy-learn:latest".into(),
            container_port,
            host_port: None,
            host_directory: "/tmp/fluensy-data".into(),
            container_directory: "/data".into(),
            network_name: "fluensy-learn-net".into(),
            network_isolation: true,
            auth,
        }
    }

    fn gateway_config_with_native_audio(
        native_audio_transcription: NativeAudioTranscriptionConfig,
    ) -> GatewayConfig {
        GatewayConfig {
            port: 8421,
            host: "127.0.0.1".into(),
            token_db_path: PathBuf::from("/tmp/brain3-test.db"),
            oauth: OAuthConfig {
                client_id: "brain3-oauth2-client".into(),
                client_secret: "secret".into(),
                access_token_lifetime_secs: 3600,
                refresh_token_lifetime_secs: 90 * 24 * 60 * 60,
                pkce_required: true,
                username: "admin".into(),
                password: "password".into(),
            },
            mcp_reverse_proxy: MCPReverseProxyConfig {
                mcp_upstream_url: "http://127.0.0.1:8420".into(),
                upstream_secret: "secret".into(),
            },
            hostname_validation: HostnameValidationConfig {
                expected_host: None,
                enforce: true,
            },
            access_mode: AccessMode::Both,
            local_mcp: None,
            container: None,
            tunnel: None,
            native_audio_transcription,
        }
    }
}
