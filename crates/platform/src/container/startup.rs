use std::sync::Arc;

use brain3_core::application::ensure_container::EnsureContainerUseCase;
use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::{
    BindMount, ContainerConfig, ContainerRuntime, ContainerStartupConfig, PortMapping,
};
use brain3_core::ports::container::ContainerPort;

use super::{DockerContainerAdapter, MacOsContainerAdapter};

pub async fn ensure_mcp_container(startup: &ContainerStartupConfig) -> Result<(), ContainerError> {
    tracing::info!(
        container = %startup.container_name,
        image = %startup.image,
        vault = %startup.vault_path.display(),
        host_port = startup.host_port,
        "ensuring MCP container is running"
    );

    let port: Arc<dyn ContainerPort> = match startup.runtime {
        ContainerRuntime::Docker => Arc::new(DockerContainerAdapter),
        ContainerRuntime::MacOSContainer => Arc::new(MacOsContainerAdapter),
    };

    let uid_gid = format!(
        "{}:{}",
        unsafe { libc::getuid() },
        unsafe { libc::getgid() }
    );

    let config = ContainerConfig {
        image: startup.image.clone(),
        name: startup.container_name.clone(),
        port_mappings: vec![PortMapping {
            host_address: "127.0.0.1".into(),
            host_port: startup.host_port,
            container_port: 8420,
        }],
        env_vars: vec![
            ("VAULT_MCP_HOST".into(), "0.0.0.0".into()),
            ("VAULT_PATH".into(), "/vault".into()),
            (
                "UPSTREAM_SHARED_SECRET_FILE".into(),
                "/run/brain3/upstream_secret".into(),
            ),
        ],
        bind_mounts: vec![
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
        ],
        user: Some(uid_gid),
        detach: true,
        remove_on_exit: false,
    };

    EnsureContainerUseCase::new(port).ensure(&config).await?;
    Ok(())
}
