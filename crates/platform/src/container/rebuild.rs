use std::env;

use anyhow::{bail, Context, Result};
use brain3_core::domain::model::{
    ContainerNetworkIsolationStrategy, ContainerRuntime, GatewayConfig,
};
use tokio::process::Command;

use super::startup::container_port_for_runtime;

pub(crate) async fn rebuild_vault_tools_image(config: &mut GatewayConfig) -> Result<()> {
    let container = config
        .container
        .as_mut()
        .context("--rebuild-container requires B3_CONTAINER_RUNTIME in your Brain3 config")?;
    if container.dev_mount_source.is_some() {
        bail!("unset B3_DEV_CONTAINER_MOUNT_SOURCE to run the rebuilt image");
    }

    let upstream = config
        .mcp_reverse_proxy
        .mcp_upstream_url
        .trim_end_matches('/');
    if container.isolation_strategy != Some(ContainerNetworkIsolationStrategy::DiscoverContainerIp)
        && upstream != format!("http://127.0.0.1:{}", container.host_port)
        && upstream != format!("http://localhost:{}", container.host_port)
    {
        bail!("unset B3_OAUTH2_GATEWAY_MCP_UPSTREAM_URL to route to the rebuilt container");
    }

    let context = env::current_dir()
        .context("could not resolve the Brain3 checkout directory")?
        .join("brain3-mcp-vault-tools");
    let containerfile = context.join("Containerfile");
    let script = context.join("scripts/build-container.sh");
    if !containerfile.is_file() || !script.is_file() {
        bail!("run --rebuild-container from the root of a Brain3 source checkout");
    }

    let runtime = match container.runtime {
        ContainerRuntime::Docker => "docker",
        ContainerRuntime::MacOSContainer => "macos-container",
    };
    let image = format!(
        "localhost/brain3-mcp-vault-tools:dev-{:032x}",
        rand::random::<u128>()
    );
    let mut build = Command::new(&script);
    build
        .args(["--container-runtime", runtime])
        .env("IMAGE_NAME", &image)
        .env("CONTAINERFILE_PATH", &containerfile)
        .env_remove("BASH_ENV")
        .kill_on_drop(true);
    // Loading the gateway config populates the process environment.
    for (key, _) in env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"B3_") || key == "LOCAL_GATEWAY_MCP_BEARER_TOKEN" {
            build.env_remove(key);
        }
    }

    tracing::info!(%image, %runtime, context = %context.display(), "building vault-tools image");
    let status = build
        .status()
        .await
        .with_context(|| format!("could not start {}", script.display()))?;
    if !status.success() {
        bail!("vault-tools image build failed ({status})");
    }

    let port = container_port_for_runtime(container.runtime);
    if !port.image_exists(&image).await? {
        bail!("the build did not produce {image} in the {runtime} image store");
    }
    container.image = image;
    Ok(())
}
