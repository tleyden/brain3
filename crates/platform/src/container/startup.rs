use std::path::Path;
use std::sync::Arc;

use brain3_core::application::ensure_container::EnsureContainerUseCase;
use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::{
    BindMount, ContainerConfig, ContainerLabel, ContainerRuntime, ContainerStartupConfig,
    ManagedContainerInfo, ManagedContainerScope, PortMapping, BRAIN3_INSTALLATION_ID_LABEL_KEY,
    BRAIN3_MANAGED_LABEL_KEY, BRAIN3_MANAGED_LABEL_VALUE, BRAIN3_MCP_ROLE_LABEL_VALUE,
    BRAIN3_ROLE_LABEL_KEY,
};
use brain3_core::domain::setup::RuntimeStartupPolicy;
use brain3_core::ports::container::{ContainerId, ContainerPort};

use super::{DockerContainerAdapter, MacOsContainerAdapter};

const DEV_MOUNT_TARGET: &str = "/workspace/brain3-mcp-vault-tools";

#[cfg(not(test))]
const GC_POLL_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_millis(200);
#[cfg(not(test))]
const GC_POLL_TIMEOUT: tokio::time::Duration = tokio::time::Duration::from_secs(5);

#[cfg(test)]
const GC_POLL_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_millis(10);
#[cfg(test)]
const GC_POLL_TIMEOUT: tokio::time::Duration = tokio::time::Duration::from_millis(100);

pub fn installation_scope_id(app_home: &Path, env_file: &Path) -> String {
    let app_home = normalize_scope_path(app_home);
    let env_file = normalize_scope_path(env_file);
    let scope = format!("app_home={app_home}\nenv_file={env_file}");

    format!("b3-{:016x}", fnv1a64(scope.as_bytes()))
}

fn normalize_scope_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

pub async fn ensure_mcp_container(
    startup: &ContainerStartupConfig,
    startup_policy: RuntimeStartupPolicy,
    installation_id: &str,
) -> Result<Option<String>, ContainerError> {
    let dev_mode = startup.dev_mount_source.is_some();
    tracing::info!(
        container = %startup.container_name,
        image = %startup.image,
        vault = %startup.vault_path.display(),
        host_port = startup.host_port,
        installation_id,
        upstream_secret_configured = !startup.upstream_secret.is_empty(),
        dev_mode,
        "ensuring MCP container is running"
    );
    tracing::info!(
        container = %startup.container_name,
        network_isolated = startup.isolation_strategy.is_some(),
        isolation_strategy = ?startup.isolation_strategy,
        startup_policy = ?startup_policy,
        "resolved MCP container network isolation mode"
    );

    let port = container_port_for_runtime(startup.runtime);
    maybe_handle_managed_container_orphans(port.as_ref(), startup, startup_policy, installation_id)
        .await?;

    let config = build_container_config(startup, installation_id);
    let (_id, container_ip) = EnsureContainerUseCase::new(port).ensure(&config).await?;
    Ok(container_ip)
}

pub async fn stop_mcp_container(startup: &ContainerStartupConfig) -> Result<(), ContainerError> {
    let port = container_port_for_runtime(startup.runtime);
    let id = ContainerId(startup.container_name.clone());

    if !port.exists(&id).await? {
        tracing::debug!(container = %startup.container_name, "managed MCP container already absent during shutdown");
        return Ok(());
    }

    if !port.is_running(&id).await? {
        tracing::debug!(container = %startup.container_name, "managed MCP container already stopped during shutdown");
        return Ok(());
    }

    tracing::info!(container = %startup.container_name, runtime = ?startup.runtime, "stopping managed MCP container during shutdown");
    port.stop(&id).await
}

fn container_port_for_runtime(runtime: ContainerRuntime) -> Arc<dyn ContainerPort> {
    match runtime {
        ContainerRuntime::Docker => Arc::new(DockerContainerAdapter),
        ContainerRuntime::MacOSContainer => Arc::new(MacOsContainerAdapter),
    }
}

fn managed_container_scope(installation_id: &str) -> ManagedContainerScope {
    ManagedContainerScope::mcp(installation_id.to_string())
}

fn managed_container_labels(installation_id: &str) -> Vec<ContainerLabel> {
    vec![
        ContainerLabel {
            key: BRAIN3_MANAGED_LABEL_KEY.into(),
            value: BRAIN3_MANAGED_LABEL_VALUE.into(),
        },
        ContainerLabel {
            key: BRAIN3_ROLE_LABEL_KEY.into(),
            value: BRAIN3_MCP_ROLE_LABEL_VALUE.into(),
        },
        ContainerLabel {
            key: BRAIN3_INSTALLATION_ID_LABEL_KEY.into(),
            value: installation_id.to_string(),
        },
    ]
}

fn build_container_config(
    startup: &ContainerStartupConfig,
    installation_id: &str,
) -> ContainerConfig {
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
            "B3_UPSTREAM_SHARED_SECRET".into(),
            startup.upstream_secret.clone(),
        ),
    ];
    if startup.isolation_strategy.is_some() {
        env_vars.push(("B3_VAULT_MCP_ALLOW_SELF_IP_HOSTS".into(), "true".into()));
    }
    if let Some(ref level) = startup.mcp_log_level {
        env_vars.push(("B3_VAULT_MCP_LOG_LEVEL".into(), level.clone()));
    }

    let mut bind_mounts = vec![BindMount {
        host_path: startup.vault_path.clone(),
        container_path: "/vault".into(),
        readonly: false,
    }];

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
    let allow_self_ip_hosts = env_vars
        .iter()
        .find(|(key, _)| key == "B3_VAULT_MCP_ALLOW_SELF_IP_HOSTS")
        .map(|(_, value)| value.as_str());
    tracing::info!(
        container = %startup.container_name,
        installation_id,
        network_isolated = startup.isolation_strategy.is_some(),
        isolation_strategy = ?startup.isolation_strategy,
        host_probe_target = %format!("127.0.0.1:{}", startup.host_port),
        isolated_probe_target = %format!("<container-ip>:{}", startup.container_port),
        allowed_hosts_env = ?allowed_hosts_env,
        allow_self_ip_hosts = ?allow_self_ip_hosts,
        "prepared MCP container runtime networking configuration"
    );

    ContainerConfig {
        image: startup.image.clone(),
        name: startup.container_name.clone(),
        isolation_strategy: startup.isolation_strategy,
        network_name: startup.network_name.clone(),
        port_mappings: vec![PortMapping {
            host_address: "127.0.0.1".into(),
            host_port: startup.host_port,
            container_port: startup.container_port,
        }],
        env_vars,
        labels: managed_container_labels(installation_id),
        bind_mounts,
        user: Some(uid_gid),
        detach: true,
        remove_on_exit: matches!(startup.runtime, ContainerRuntime::Docker),
        workdir,
        command,
    }
}

async fn maybe_handle_managed_container_orphans(
    port: &dyn ContainerPort,
    startup: &ContainerStartupConfig,
    startup_policy: RuntimeStartupPolicy,
    installation_id: &str,
) -> Result<(), ContainerError> {
    if !startup_policy.checks_for_orphans() {
        tracing::debug!(
            container = %startup.container_name,
            installation_id,
            "skipping managed-container orphan preflight during setup or reconfiguration"
        );
        return Ok(());
    }

    let scope = managed_container_scope(installation_id);
    let containers = port.list_managed_containers(&scope).await?;
    if containers.is_empty() {
        tracing::debug!(
            installation_id,
            "no managed orphan containers found for this installation scope"
        );
        return Ok(());
    }

    if !startup_policy.gc_containers_enabled() {
        tracing::warn!(
            installation_id,
            containers = ?containers,
            "managed orphan containers detected; refusing cleanup without explicit startup approval"
        );
        return Err(
            ContainerError::orphaned_managed_containers_for_installation(
                installation_id.to_string(),
                containers,
            ),
        );
    }

    garbage_collect_managed_containers(port, startup.runtime, installation_id, containers).await
}

async fn garbage_collect_managed_containers(
    port: &dyn ContainerPort,
    runtime: ContainerRuntime,
    installation_id: &str,
    containers: Vec<ManagedContainerInfo>,
) -> Result<(), ContainerError> {
    for container in containers {
        let id = ContainerId(container.name.clone());

        if container.running {
            tracing::warn!(
                installation_id,
                container = %container.name,
                state = %container.state,
                "stopping managed orphan MCP container before removal"
            );
            match port.stop(&id).await {
                Ok(()) => {}
                Err(ContainerError::CommandFailed { ref stderr, .. })
                    if stderr.contains("No such container") =>
                {
                    tracing::debug!(
                        installation_id,
                        container = %container.name,
                        "orphan container already gone during stop — skipping"
                    );
                    continue;
                }
                Err(error) => {
                    return Err(ContainerError::Other(format!(
                        "failed to stop managed orphan container '{}': {}",
                        container.name,
                        error.summary()
                    )));
                }
            }
        } else {
            tracing::info!(
                installation_id,
                container = %container.name,
                state = %container.state,
                "removing stopped managed orphan MCP container"
            );
        }

        match runtime {
            ContainerRuntime::Docker => {
                // Docker containers use --rm, so the daemon removes the container
                // automatically after stop. Poll until it disappears; fall back to
                // explicit docker rm only if it is still present after the timeout
                // (e.g. a stopped container whose --rm somehow did not fire).
                let gone = wait_for_container_gone(
                    port,
                    &id,
                    installation_id,
                    GC_POLL_INTERVAL,
                    GC_POLL_TIMEOUT,
                )
                .await?;
                if gone {
                    tracing::info!(
                        installation_id,
                        container = %container.name,
                        "removed managed orphan MCP container (Docker --rm)"
                    );
                    continue;
                }
                tracing::debug!(
                    installation_id,
                    container = %container.name,
                    "container still present after polling — falling back to explicit docker rm"
                );
            }
            ContainerRuntime::MacOSContainer => {
                // macOS containers do not use --rm; explicit removal is always needed.
            }
        }

        match port.remove(&id).await {
            Ok(()) => {}
            Err(ContainerError::CommandFailed { ref stderr, .. })
                if stderr.contains("No such container") =>
            {
                tracing::debug!(
                    installation_id,
                    container = %container.name,
                    "orphan container already gone during remove — skipping"
                );
                continue;
            }
            Err(error) => {
                return Err(ContainerError::Other(format!(
                    "failed to remove managed orphan container '{}': {}",
                    container.name,
                    error.summary()
                )));
            }
        }
        tracing::info!(
            installation_id,
            container = %container.name,
            "removed managed orphan MCP container"
        );
    }

    Ok(())
}

/// Polls `port.exists()` until the container disappears or the timeout elapses.
/// Returns `true` if the container is gone, `false` if it is still present at timeout.
async fn wait_for_container_gone(
    port: &dyn ContainerPort,
    id: &ContainerId,
    installation_id: &str,
    poll_interval: tokio::time::Duration,
    timeout: tokio::time::Duration,
) -> Result<bool, ContainerError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut attempt = 0u32;
    loop {
        if !port.exists(id).await? {
            tracing::debug!(
                installation_id,
                container = %id.0,
                attempts = attempt,
                "container removed by Docker --rm"
            );
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::debug!(
                installation_id,
                container = %id.0,
                "container still present after polling timeout"
            );
            return Ok(false);
        }
        attempt += 1;
        tokio::time::sleep(poll_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use brain3_core::domain::model::{ContainerNetworkIsolationStrategy, ManagedContainerInfo};
    use brain3_core::ports::container::NetworkPreparation;

    use super::*;

    #[derive(Debug, Clone, Default)]
    struct MockState {
        managed_containers: Vec<ManagedContainerInfo>,
        stop_calls: Vec<String>,
        remove_calls: Vec<String>,
        exists_calls: Vec<String>,
        /// Responses returned by `exists()` in order; cycles `false` once exhausted.
        exists_responses: Vec<bool>,
    }

    struct MockContainerPort {
        state: Mutex<MockState>,
    }

    impl MockContainerPort {
        fn new(state: MockState) -> Self {
            Self {
                state: Mutex::new(state),
            }
        }

        fn snapshot(&self) -> MockState {
            self.state
                .lock()
                .expect("lock should not be poisoned")
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl ContainerPort for MockContainerPort {
        async fn image_exists(&self, _image: &str) -> Result<bool, ContainerError> {
            Ok(true)
        }

        async fn pull_image(&self, _image: &str) -> Result<(), ContainerError> {
            Ok(())
        }

        async fn exists(&self, id: &ContainerId) -> Result<bool, ContainerError> {
            let mut s = self.state.lock().expect("lock should not be poisoned");
            s.exists_calls.push(id.0.clone());
            let result = if s.exists_responses.is_empty() {
                false
            } else {
                s.exists_responses.remove(0)
            };
            Ok(result)
        }

        async fn is_running(&self, _id: &ContainerId) -> Result<bool, ContainerError> {
            Ok(false)
        }

        async fn logs_tail(
            &self,
            _id: &ContainerId,
            _lines: usize,
        ) -> Result<String, ContainerError> {
            Ok(String::new())
        }

        async fn ensure_internal_network(
            &self,
            _network_name: &str,
        ) -> Result<NetworkPreparation, ContainerError> {
            Ok(NetworkPreparation::Created)
        }

        async fn get_container_ip(
            &self,
            _id: &ContainerId,
        ) -> Result<Option<String>, ContainerError> {
            Ok(None)
        }

        async fn list_managed_containers(
            &self,
            _scope: &ManagedContainerScope,
        ) -> Result<Vec<ManagedContainerInfo>, ContainerError> {
            Ok(self
                .state
                .lock()
                .expect("lock should not be poisoned")
                .managed_containers
                .clone())
        }

        async fn run(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError> {
            Ok(ContainerId(config.name.clone()))
        }

        async fn stop(&self, id: &ContainerId) -> Result<(), ContainerError> {
            self.state
                .lock()
                .expect("lock should not be poisoned")
                .stop_calls
                .push(id.0.clone());
            Ok(())
        }

        async fn remove(&self, id: &ContainerId) -> Result<(), ContainerError> {
            self.state
                .lock()
                .expect("lock should not be poisoned")
                .remove_calls
                .push(id.0.clone());
            Ok(())
        }
    }

    fn sample_startup() -> ContainerStartupConfig {
        ContainerStartupConfig {
            runtime: ContainerRuntime::Docker,
            image: "ghcr.io/tleyden/brain3-mcp-vault-tools:v0.2.3".into(),
            container_name: "brain3-mcp-vault-tools".into(),
            network_name: "brain3-mcp-net".into(),
            vault_path: "/tmp/vault".into(),
            upstream_secret: "secret".into(),
            host_port: 2765,
            container_port: 2765,
            isolation_strategy: Some(ContainerNetworkIsolationStrategy::DiscoverContainerIp),
            dev_mount_source: None,
            mcp_log_level: None,
        }
    }

    #[test]
    fn installation_scope_id_changes_with_env_scope() {
        let first = installation_scope_id(
            Path::new("/tmp/brain3-home-a"),
            Path::new("/tmp/brain3-home-a/.env"),
        );
        let second = installation_scope_id(
            Path::new("/tmp/brain3-home-a"),
            Path::new("/tmp/brain3-home-b/.env"),
        );

        assert_ne!(first, second);
    }

    #[test]
    fn build_container_config_adds_brain3_labels() {
        let config = build_container_config(&sample_startup(), "scope-1");
        assert_eq!(
            config.labels,
            vec![
                ContainerLabel {
                    key: BRAIN3_MANAGED_LABEL_KEY.into(),
                    value: BRAIN3_MANAGED_LABEL_VALUE.into(),
                },
                ContainerLabel {
                    key: BRAIN3_ROLE_LABEL_KEY.into(),
                    value: BRAIN3_MCP_ROLE_LABEL_VALUE.into(),
                },
                ContainerLabel {
                    key: BRAIN3_INSTALLATION_ID_LABEL_KEY.into(),
                    value: "scope-1".into(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn orphan_preflight_requires_explicit_gc() {
        let port = MockContainerPort::new(MockState {
            managed_containers: vec![ManagedContainerInfo {
                name: "brain3-old".into(),
                running: true,
                state: "running".into(),
                labels: managed_container_labels("scope-1"),
            }],
            ..Default::default()
        });

        let error = maybe_handle_managed_container_orphans(
            &port,
            &sample_startup(),
            RuntimeStartupPolicy::configured(false),
            "scope-1",
        )
        .await
        .expect_err("orphan preflight should fail closed without explicit gc");

        assert!(error.requires_explicit_gc());
        assert_eq!(port.snapshot().stop_calls, Vec::<String>::new());
        assert_eq!(port.snapshot().remove_calls, Vec::<String>::new());
    }

    #[tokio::test]
    async fn gc_docker_stops_running_and_waits_for_auto_removal() {
        // Docker runtime: after stop, --rm removes the container.
        // exists() returns false immediately → no explicit docker rm.
        let port = MockContainerPort::new(MockState {
            managed_containers: vec![
                ManagedContainerInfo {
                    name: "brain3-running".into(),
                    running: true,
                    state: "running".into(),
                    labels: managed_container_labels("scope-1"),
                },
                ManagedContainerInfo {
                    name: "brain3-exited".into(),
                    running: false,
                    state: "exited".into(),
                    labels: managed_container_labels("scope-1"),
                },
            ],
            exists_responses: vec![], // all calls return false (already gone)
            ..Default::default()
        });

        maybe_handle_managed_container_orphans(
            &port,
            &sample_startup(), // Docker runtime
            RuntimeStartupPolicy::configured(true),
            "scope-1",
        )
        .await
        .expect("gc should remove scoped managed orphans");

        let state = port.snapshot();
        assert_eq!(state.stop_calls, vec!["brain3-running".to_string()]);
        assert_eq!(state.remove_calls, Vec::<String>::new(), "docker rm should not be called when --rm handles removal");
    }

    #[tokio::test]
    async fn gc_docker_falls_back_to_explicit_rm_when_poll_times_out() {
        // Docker runtime: exists() keeps returning true (--rm didn't fire).
        // After timeout, we fall back to explicit docker rm.
        let port = MockContainerPort::new(MockState {
            managed_containers: vec![ManagedContainerInfo {
                name: "brain3-stuck".into(),
                running: false,
                state: "exited".into(),
                labels: managed_container_labels("scope-1"),
            }],
            // Always exists — simulates --rm not firing (enough for test timeout)
            exists_responses: vec![true; 20],
            ..Default::default()
        });

        let containers = vec![ManagedContainerInfo {
            name: "brain3-stuck".into(),
            running: false,
            state: "exited".into(),
            labels: managed_container_labels("scope-1"),
        }];

        garbage_collect_managed_containers(
            &port,
            ContainerRuntime::Docker,
            "scope-1",
            containers,
        )
        .await
        .expect("gc should fall back to explicit rm on timeout");

        let state = port.snapshot();
        assert_eq!(state.remove_calls, vec!["brain3-stuck".to_string()]);
    }

    #[tokio::test]
    async fn gc_macos_always_calls_explicit_remove() {
        // macOS runtime: no --rm, explicit remove always called.
        let mut startup = sample_startup();
        startup.runtime = ContainerRuntime::MacOSContainer;

        let port = MockContainerPort::new(MockState {
            managed_containers: vec![ManagedContainerInfo {
                name: "brain3-macos".into(),
                running: true,
                state: "running".into(),
                labels: managed_container_labels("scope-1"),
            }],
            ..Default::default()
        });

        maybe_handle_managed_container_orphans(
            &port,
            &startup,
            RuntimeStartupPolicy::configured(true),
            "scope-1",
        )
        .await
        .expect("gc should explicitly remove macOS container");

        let state = port.snapshot();
        assert_eq!(state.stop_calls, vec!["brain3-macos".to_string()]);
        assert_eq!(state.remove_calls, vec!["brain3-macos".to_string()]);
        assert!(state.exists_calls.is_empty(), "exists should not be polled for macOS");
    }
}
