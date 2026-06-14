use std::sync::Arc;

use brain3_core::application::ensure_container::EnsureContainerUseCase;
use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::{
    BindMount, ContainerConfig, ContainerRuntime, ContainerStartupConfig, PortMapping,
};
use brain3_core::ports::container::ContainerPort;

use super::{DockerContainerAdapter, MacOsContainerAdapter};

const DEV_MOUNT_TARGET: &str = "/workspace/brain3-mcp-vault-tools";

pub async fn ensure_mcp_container(
    startup: &ContainerStartupConfig,
) -> Result<Option<String>, ContainerError> {
    let dev_mode = startup.dev_mount_source.is_some();
    tracing::info!(
        container = %startup.container_name,
        image = %startup.image,
        vault = %startup.vault_path.display(),
        host_port = startup.host_port,
        upstream_secret_dir = %startup.upstream_secret_dir.display(),
        dev_mode,
        "ensuring MCP container is running"
    );
    tracing::info!(
        container = %startup.container_name,
        network_isolated = startup.network_isolated,
        "resolved MCP container network isolation mode"
    );

    let port: Arc<dyn ContainerPort> = match startup.runtime {
        ContainerRuntime::Docker => Arc::new(DockerContainerAdapter),
        ContainerRuntime::MacOSContainer => Arc::new(MacOsContainerAdapter),
    };

    let uid_gid = format!("{}:{}", unsafe { libc::getuid() }, unsafe {
        libc::getgid()
    });

    let mut env_vars = vec![
        ("B3_VAULT_MCP_HOST".into(), "0.0.0.0".into()),
        (
            "B3_VAULT_MCP_PORT".into(),
            startup.container_port.to_string(),
        ),
        ("B3_VAULT_PATH".into(), "/vault".into()),
        (
            "B3_UPSTREAM_SHARED_SECRET_FILE".into(),
            "/run/brain3/upstream_secret".into(),
        ),
    ];

    let mut bind_mounts = vec![
        BindMount {
            host_path: startup.vault_path.clone(),
            container_path: "/vault".into(),
            readonly: false,
        },
        BindMount {
            host_path: startup.upstream_secret_dir.clone(),
            container_path: "/run/brain3".into(),
            readonly: true,
        },
    ];

    let mut workdir = None;
    let mut command = Vec::new();

    if let Some(ref source_path) = startup.dev_mount_source {
        bind_mounts.push(BindMount {
            host_path: source_path.clone(),
            container_path: DEV_MOUNT_TARGET.into(),
            readonly: true,
        });
        env_vars.push(("PYTHONPATH".into(), format!("{DEV_MOUNT_TARGET}/src")));
        workdir = Some(DEV_MOUNT_TARGET.to_string());
        command = vec![
            "/opt/brain3-mcp-vault-tools/.venv/bin/python".into(),
            "-m".into(),
            "obsidian_mcp_server.server".into(),
        ];
    }

    let allowed_hosts_env = env_vars
        .iter()
        .find(|(key, _)| key == "B3_VAULT_MCP_ALLOWED_HOSTS")
        .map(|(_, value)| value.as_str());
    tracing::info!(
        container = %startup.container_name,
        network_isolated = startup.network_isolated,
        host_probe_target = %format!("127.0.0.1:{}", startup.host_port),
        isolated_probe_target = %format!("<container-ip>:{}", startup.container_port),
        allowed_hosts_env = ?allowed_hosts_env,
        "prepared MCP container runtime networking configuration"
    );

    let config = ContainerConfig {
        image: startup.image.clone(),
        name: startup.container_name.clone(),
        network_isolated: startup.network_isolated,
        port_mappings: vec![PortMapping {
            host_address: "127.0.0.1".into(),
            host_port: startup.host_port,
            container_port: startup.container_port,
        }],
        env_vars,
        bind_mounts,
        user: Some(uid_gid),
        detach: true,
        remove_on_exit: false,
        workdir,
        command,
    };

    let (_id, container_ip) = EnsureContainerUseCase::new(port).ensure(&config).await?;
    Ok(container_ip)
}
