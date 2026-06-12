use std::sync::Arc;

use anyhow::{bail, Context, Result};
use brain3_core::domain::model::{GatewayConfig, TunnelConfig};
use brain3_core::domain::setup::RuntimeLaunchPlan;
use brain3_core::ports::tunnel::TunnelPort;

use crate::config::{log_config, upstream_secret};
use crate::container::startup::ensure_mcp_container;
use crate::tunnel::start_tunnel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStatus {
    NotConfigured,
    Started,
}

pub struct RuntimeBootstrap {
    pub config: Arc<GatewayConfig>,
    pub upstream_secret: String,
    pub launch_plan: RuntimeLaunchPlan,
    pub public_url: Option<String>,
    pub container_status: StartupStatus,
    pub tunnel_status: StartupStatus,
    _tunnel_guard: Option<Box<dyn TunnelPort>>,
}

impl RuntimeBootstrap {
    pub fn display_url(&self, local_url: &str) -> String {
        self.public_url
            .clone()
            .unwrap_or_else(|| local_url.to_string())
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

    let container_status = if let Some(startup) = &config.container {
        ensure_mcp_container(startup)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("failed to start MCP container")?;
        StartupStatus::Started
    } else {
        StartupStatus::NotConfigured
    };

    let (tunnel_status, public_url, tunnel_guard) = if let Some(tunnel_config) = &config.tunnel {
        let (adapter, info) = start_tunnel(tunnel_config)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("failed to start tunnel")?;
        tracing::info!(url = %info.public_url, "tunnel started");
        (StartupStatus::Started, Some(info.public_url), Some(adapter))
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
        _tunnel_guard: tunnel_guard,
    })
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
