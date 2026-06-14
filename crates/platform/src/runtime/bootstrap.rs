use std::sync::Arc;

use anyhow::{bail, Result};
use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::{GatewayConfig, TunnelConfig};
use brain3_core::domain::setup::RuntimeLaunchPlan;
use brain3_core::ports::tunnel::TunnelPort;

use crate::config::{log_config, upstream_secret};
use crate::container::startup::ensure_mcp_container;
use crate::tunnel::start_tunnel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupStatus {
    NotConfigured,
    Ready,
    Failed { summary: String },
}

impl StartupStatus {
    pub fn failure_summary(&self) -> Option<&str> {
        match self {
            Self::Failed { summary } => Some(summary.as_str()),
            _ => None,
        }
    }

    pub fn allows_gateway_start(&self) -> bool {
        !matches!(self, Self::Failed { .. })
    }
}

pub struct RuntimeBootstrap {
    pub config: Arc<GatewayConfig>,
    pub upstream_secret: String,
    pub launch_plan: RuntimeLaunchPlan,
    pub public_url: Option<String>,
    pub container_status: StartupStatus,
    pub tunnel_status: StartupStatus,
    tunnel: Option<Box<dyn TunnelPort>>,
}

impl RuntimeBootstrap {
    pub fn new(
        config: Arc<GatewayConfig>,
        upstream_secret: String,
        launch_plan: RuntimeLaunchPlan,
        public_url: Option<String>,
        container_status: StartupStatus,
        tunnel_status: StartupStatus,
    ) -> Self {
        Self {
            config,
            upstream_secret,
            launch_plan,
            public_url,
            container_status,
            tunnel_status,
            tunnel: None,
        }
    }

    pub async fn stop_tunnel(&mut self) {
        if let Some(tunnel) = self.tunnel.take() {
            if let Err(e) = tunnel.stop().await {
                tracing::warn!(error = %e, "error stopping tunnel during shutdown");
            }
        }
    }

    pub fn display_url(&self, local_url: &str) -> String {
        self.public_url
            .clone()
            .unwrap_or_else(|| local_url.to_string())
    }

    pub fn can_start_gateway(&self) -> bool {
        self.container_status.allows_gateway_start()
    }

    pub fn primary_failure_summary(&self) -> Option<&str> {
        self.container_status
            .failure_summary()
            .or_else(|| self.tunnel_status.failure_summary())
    }
}

pub fn named_tunnel_setup_config(config: &GatewayConfig) -> Option<&TunnelConfig> {
    match &config.tunnel {
        Some(tc @ TunnelConfig::CloudflareNamed { config_file, .. }) if !config_file.exists() => {
            Some(tc)
        }
        _ => None,
    }
}

pub async fn bootstrap_configured_runtime(
    config: Arc<GatewayConfig>,
    launch_plan: RuntimeLaunchPlan,
) -> Result<RuntimeBootstrap> {
    log_tunnel_mode(&config);
    ensure_named_tunnel_config_exists(&config)?;
    log_config::log_startup_config(&config);

    let upstream_secret =
        upstream_secret::read_or_create(&config.mcp_reverse_proxy.upstream_secret_file)?;

    let mut config = config;
    let container_status = if let Some(startup) = &config.container {
        match ensure_mcp_container(startup).await {
            Ok(Some(container_ip)) if startup.network_isolated => {
                let upstream_url = format!("http://{}:{}", container_ip, startup.container_port);
                tracing::info!(
                    container_ip = %container_ip,
                    upstream_url = %upstream_url,
                    "isolated container: routing MCP reverse proxy directly to container IP"
                );
                let mut updated = (*config).clone();
                updated.mcp_reverse_proxy.mcp_upstream_url = upstream_url;
                config = Arc::new(updated);
                StartupStatus::Ready
            }
            Ok(_) => StartupStatus::Ready,
            Err(error) => container_failure_status(startup.container_name.as_str(), &error),
        }
    } else {
        StartupStatus::NotConfigured
    };

    let (tunnel_status, public_url, tunnel_guard) = if !container_status.allows_gateway_start() {
        match &config.tunnel {
            Some(_) => (
                StartupStatus::Failed {
                    summary:
                        "Tunnel not started because the MCP container failed startup verification"
                            .into(),
                },
                None,
                None,
            ),
            None => (StartupStatus::NotConfigured, None, None),
        }
    } else if let Some(tunnel_config) = &config.tunnel {
        match start_tunnel(tunnel_config).await {
            Ok((adapter, info)) => {
                tracing::info!(url = %info.public_url, "tunnel started");
                (StartupStatus::Ready, Some(info.public_url), Some(adapter))
            }
            Err(error) => {
                tracing::error!(error = %error, "failed to start tunnel");
                (
                    StartupStatus::Failed {
                        summary: error.to_string(),
                    },
                    None,
                    None,
                )
            }
        }
    } else {
        (StartupStatus::NotConfigured, None, None)
    };

    Ok(RuntimeBootstrap {
        config,
        upstream_secret,
        launch_plan,
        public_url,
        container_status,
        tunnel_status,
        tunnel: tunnel_guard,
    })
}

fn container_failure_status(container_name: &str, error: &ContainerError) -> StartupStatus {
    let summary = error.summary();
    if let Some(logs) = error.recent_logs() {
        tracing::error!(container = container_name, summary, logs = %logs, "MCP container startup failed");
    } else {
        tracing::error!(
            container = container_name,
            summary,
            "MCP container startup failed"
        );
    }

    StartupStatus::Failed { summary }
}

fn log_tunnel_mode(config: &GatewayConfig) {
    match &config.tunnel {
        Some(TunnelConfig::CloudflareQuick { local_port }) => {
            tracing::info!(local_port = %local_port, "tunnel mode: Cloudflare quick tunnel");
        }
        Some(TunnelConfig::CloudflareNamed {
            tunnel_name,
            domain,
            config_file,
            ..
        }) => {
            tracing::info!(
                tunnel_name = %tunnel_name,
                domain = %domain,
                config_file = %config_file.display(),
                "tunnel mode: Cloudflare named tunnel"
            );
        }
        None => {
            tracing::info!("tunnel mode: none (no public ingress configured)");
        }
    }
}

fn ensure_named_tunnel_config_exists(config: &GatewayConfig) -> Result<()> {
    let Some(TunnelConfig::CloudflareNamed {
        config_file,
        tunnel_name,
        ..
    }) = named_tunnel_setup_config(config)
    else {
        return Ok(());
    };

    eprintln!(
        "\nERROR: Cloudflare named tunnel not yet provisioned.\n\
         \n  Config file not found: {}\
         \n\n  Run this in an interactive terminal:\n    brain3 --cf-setup\
         \n\n  Or use a quick tunnel instead (no setup needed):\n    Set B3_CF_QUICK_TUNNEL=true in .env (and remove B3_CF_TUNNEL_NAME/B3_CF_DOMAIN)\n",
        config_file.display()
    );
    tracing::error!(
        config_file = %config_file.display(),
        tunnel_name = %tunnel_name,
        "named tunnel config file not found — run: brain3 --cf-setup"
    );
    bail!(
        "named tunnel config file not found: {}",
        config_file.display()
    );
}
