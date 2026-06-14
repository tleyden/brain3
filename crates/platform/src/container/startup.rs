use std::sync::Arc;

use brain3_core::application::ensure_container::EnsureContainerUseCase;
use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::{
    BindMount, ContainerConfig, ContainerRuntime, ContainerStartupConfig, PortMapping,
};
use brain3_core::ports::container::ContainerPort;

use super::{DockerContainerAdapter, MacOsContainerAdapter};

const DEV_MOUNT_TARGET: &str = "/workspace/brain3-mcp-vault-tools";

/// In-container directory where Brain3 mounts the runtime socket directory.
const CONTAINER_RUNTIME_DIR: &str = "/run/brain3-runtime";
/// In-container path of the Unix socket the Python MCP server binds to.
const CONTAINER_SOCKET_PATH: &str = "/run/brain3-runtime/mcp.sock";

pub async fn ensure_mcp_container(startup: &ContainerStartupConfig) -> Result<(), ContainerError> {
    let dev_mode = startup.dev_mount_source.is_some();
    tracing::info!(
        container = %startup.container_name,
        image = %startup.image,
        vault = %startup.vault_path.display(),
        host_port = startup.host_port,
        upstream_secret_dir = %startup.upstream_secret_dir.display(),
        network_isolated = startup.network_isolated,
        dev_mode,
        "ensuring MCP container is running"
    );

    let port: Arc<dyn ContainerPort> = match startup.runtime {
        ContainerRuntime::Docker => Arc::new(DockerContainerAdapter),
        ContainerRuntime::MacOSContainer => Arc::new(MacOsContainerAdapter),
    };

    let uid_gid = format!("{}:{}", unsafe { libc::getuid() }, unsafe {
        libc::getgid()
    });

    let mut env_vars = vec![
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

    // In isolated mode: use a Unix socket shared via a host-mounted runtime dir.
    // In non-isolated mode: publish a TCP loopback port as before.
    let (port_mappings, unix_socket_path) = if startup.network_isolated {
        let host_runtime_dir = &startup.host_runtime_dir;
        let host_socket = host_runtime_dir.join("mcp.sock");

        tokio::fs::create_dir_all(host_runtime_dir).await.map_err(|e| {
            ContainerError::Other(format!(
                "failed to create host runtime dir {}: {e}",
                host_runtime_dir.display()
            ))
        })?;

        // Unlink any stale socket so the container's bind() always succeeds.
        if host_socket.exists() {
            tokio::fs::remove_file(&host_socket).await.map_err(|e| {
                ContainerError::Other(format!(
                    "failed to remove stale socket {}: {e}",
                    host_socket.display()
                ))
            })?;
        }

        bind_mounts.push(BindMount {
            host_path: host_runtime_dir.clone(),
            container_path: CONTAINER_RUNTIME_DIR.into(),
            readonly: false,
        });

        env_vars.push((
            "B3_VAULT_MCP_UNIX_SOCKET".into(),
            CONTAINER_SOCKET_PATH.into(),
        ));

        tracing::info!(
            host_socket = %host_socket.display(),
            container_socket = CONTAINER_SOCKET_PATH,
            "isolated mode: MCP container will serve on Unix socket"
        );

        (vec![], Some(host_socket))
    } else {
        env_vars.push(("B3_VAULT_MCP_HOST".into(), "0.0.0.0".into()));
        env_vars.push((
            "B3_VAULT_MCP_PORT".into(),
            startup.container_port.to_string(),
        ));

        let mappings = vec![PortMapping {
            host_address: "127.0.0.1".into(),
            host_port: startup.host_port,
            container_port: startup.container_port,
        }];

        tracing::info!(
            host_port = startup.host_port,
            container_port = startup.container_port,
            "non-isolated mode: MCP container will serve on loopback TCP port"
        );

        (mappings, None)
    };

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

    let config = ContainerConfig {
        image: startup.image.clone(),
        name: startup.container_name.clone(),
        network_isolated: startup.network_isolated,
        port_mappings,
        env_vars,
        bind_mounts,
        user: Some(uid_gid),
        detach: true,
        remove_on_exit: false,
        workdir,
        command,
        unix_socket_path,
    };

    EnsureContainerUseCase::new(port).ensure(&config).await?;
    Ok(())
}
