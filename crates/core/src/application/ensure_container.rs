use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::domain::errors::ContainerError;
use crate::domain::model::{ContainerConfig, ContainerNetworkIsolationStrategy, PortMapping};
use crate::ports::container::{ContainerId, ContainerPort};

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(200);
const DEFAULT_LOG_TAIL_LINES: usize = 40;
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy)]
struct StartupProbeSettings {
    timeout: Duration,
    poll_interval: Duration,
    log_tail_lines: usize,
}

impl Default for StartupProbeSettings {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_STARTUP_TIMEOUT,
            poll_interval: DEFAULT_STARTUP_POLL_INTERVAL,
            log_tail_lines: DEFAULT_LOG_TAIL_LINES,
        }
    }
}

pub struct EnsureContainerUseCase {
    port: Arc<dyn ContainerPort>,
    probe_settings: StartupProbeSettings,
}

impl EnsureContainerUseCase {
    pub fn new(port: Arc<dyn ContainerPort>) -> Self {
        Self {
            port,
            probe_settings: StartupProbeSettings::default(),
        }
    }

    #[cfg(test)]
    fn with_probe_settings(
        port: Arc<dyn ContainerPort>,
        probe_settings: StartupProbeSettings,
    ) -> Self {
        Self {
            port,
            probe_settings,
        }
    }

    pub async fn ensure(
        &self,
        config: &ContainerConfig,
    ) -> Result<(ContainerId, Option<String>), ContainerError> {
        if !self.port.image_exists(&config.image).await? {
            tracing::warn!(
                image = %config.image,
                "container image not found locally; will pull from registry"
            );
            self.port.pull_image(&config.image).await?;

            if !self.port.image_exists(&config.image).await? {
                return Err(ContainerError::ImageNotFound(config.image.clone()));
            }
        }

        let id = ContainerId(config.name.clone());

        if self.port.exists(&id).await? {
            if self.port.is_running(&id).await? {
                tracing::info!(container = %config.name, "stopping running container to pick up fresh shared secret");
                self.port.stop(&id).await?;
            }
            tracing::info!(container = %config.name, "removing container before fresh start");
            self.port.remove(&id).await?;
        }

        tracing::info!(container = %config.name, image = %config.image, "starting container");
        let runtime_config = config.clone();
        if config.isolation_strategy.is_some() {
            let isolation_ok = self
                .port
                .prepare_network_isolation(&config.network_name)
                .await?;
            if !isolation_ok {
                return Err(ContainerError::Other(format!(
                    "network isolation setup failed for '{}' — aborting to preserve security posture",
                    config.network_name
                )));
            }
        }

        let id = self.port.run(&runtime_config).await?;

        let container_ip = if runtime_config.isolation_strategy
            == Some(ContainerNetworkIsolationStrategy::DiscoverContainerIp)
        {
            self.port.get_container_ip(&id).await?
        } else {
            None
        };

        self.verify_startup(&id, config, container_ip.as_deref())
            .await?;
        tracing::info!(container = %config.name, "container ready");
        Ok((id, container_ip))
    }

    async fn verify_startup(
        &self,
        id: &ContainerId,
        config: &ContainerConfig,
        probe_host_override: Option<&str>,
    ) -> Result<(), ContainerError> {
        let deadline = Instant::now() + self.probe_settings.timeout;
        let probe_desc = if let Some(host) = probe_host_override {
            config
                .port_mappings
                .iter()
                .map(|mapping| format!("{host}:{}", mapping.container_port))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            format_port_mappings(&config.port_mappings)
        };
        tracing::info!(
            container = %config.name,
            probe_target = %probe_desc,
            probe_uses_container_ip = probe_host_override.is_some(),
            timeout_ms = self.probe_settings.timeout.as_millis() as u64,
            poll_interval_ms = self.probe_settings.poll_interval.as_millis() as u64,
            "verifying container startup reachability"
        );

        loop {
            if !self.port.is_running(id).await? {
                return Err(self
                    .startup_failed(
                        id,
                        format!(
                            "container '{}' exited during startup verification",
                            config.name
                        ),
                    )
                    .await);
            }

            let ports_ready = config.port_mappings.is_empty()
                || config.port_mappings.iter().all(|mapping| {
                    if let Some(host) = probe_host_override {
                        tcp_port_ready(host, mapping.container_port)
                    } else {
                        tcp_port_ready(&mapping.host_address, mapping.host_port)
                    }
                });

            if ports_ready {
                return Ok(());
            }

            if Instant::now() >= deadline {
                return Err(self
                    .startup_failed(
                        id,
                        format!(
                            "container '{}' did not become reachable on {} before timeout",
                            config.name, probe_desc
                        ),
                    )
                    .await);
            }

            sleep(self.probe_settings.poll_interval);
        }
    }

    async fn startup_failed(&self, id: &ContainerId, summary: String) -> ContainerError {
        let logs = match self
            .port
            .logs_tail(id, self.probe_settings.log_tail_lines)
            .await
        {
            Ok(output) => {
                let trimmed = output.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            Err(error) => {
                tracing::warn!(container = %id.0, error = %error, "failed to collect recent container logs");
                None
            }
        };

        if let Some(logs) = &logs {
            tracing::error!(container = %id.0, summary, logs = %logs, "container startup verification failed");
        } else {
            tracing::error!(container = %id.0, summary, "container startup verification failed");
        }

        ContainerError::StartupFailed { summary, logs }
    }
}

fn tcp_port_ready(host: &str, port: u16) -> bool {
    match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs
            .into_iter()
            .any(|addr| TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT).is_ok()),
        Err(_) => false,
    }
}

fn format_port_mappings(port_mappings: &[PortMapping]) -> String {
    port_mappings
        .iter()
        .map(|mapping| format!("{}:{}", mapping.host_address, mapping.host_port))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, Default)]
    struct MockState {
        image_exists: bool,
        container_exists: bool,
        container_running: bool,
        running_checks: Vec<bool>,
        logs_tail_output: Option<String>,
        prepare_network_isolation_result: bool,
        prepare_network_isolation_count: usize,
        last_run_isolation_strategy: Option<Option<ContainerNetworkIsolationStrategy>>,
        pull_count: usize,
        stop_count: usize,
        remove_count: usize,
        run_count: usize,
        logs_tail_count: usize,
        actions: Vec<&'static str>,
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
            self.state.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ContainerPort for MockContainerPort {
        async fn image_exists(&self, _image: &str) -> Result<bool, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("image_exists");
            Ok(state.image_exists)
        }

        async fn pull_image(&self, _image: &str) -> Result<(), ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("pull_image");
            state.pull_count += 1;
            state.image_exists = true;
            Ok(())
        }

        async fn exists(&self, _id: &ContainerId) -> Result<bool, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("exists");
            Ok(state.container_exists)
        }

        async fn is_running(&self, _id: &ContainerId) -> Result<bool, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("is_running");
            if !state.running_checks.is_empty() {
                state.container_running = state.running_checks.remove(0);
            }
            Ok(state.container_running)
        }

        async fn logs_tail(
            &self,
            _id: &ContainerId,
            _lines: usize,
        ) -> Result<String, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("logs_tail");
            state.logs_tail_count += 1;
            Ok(state.logs_tail_output.clone().unwrap_or_default())
        }

        async fn prepare_network_isolation(
            &self,
            _network_name: &str,
        ) -> Result<bool, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("prepare_network_isolation");
            state.prepare_network_isolation_count += 1;
            Ok(state.prepare_network_isolation_result)
        }

        async fn run(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("run");
            state.run_count += 1;
            state.last_run_isolation_strategy = Some(config.isolation_strategy);
            state.container_exists = true;
            state.container_running = true;
            Ok(ContainerId(config.name.clone()))
        }

        async fn stop(&self, _id: &ContainerId) -> Result<(), ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("stop");
            state.stop_count += 1;
            state.container_running = false;
            Ok(())
        }

        async fn remove(&self, _id: &ContainerId) -> Result<(), ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("remove");
            state.remove_count += 1;
            state.container_exists = false;
            Ok(())
        }

        async fn get_container_ip(
            &self,
            _id: &ContainerId,
        ) -> Result<Option<String>, ContainerError> {
            Ok(None)
        }
    }

    fn sample_config() -> ContainerConfig {
        ContainerConfig {
            image: "ghcr.io/tleyden/brain3-mcp-vault-tools:latest".into(),
            name: "brain3-mcp-vault-tools".into(),
            isolation_strategy: None,
            network_name: "brain3-mcp-net".into(),
            port_mappings: vec![],
            env_vars: vec![],
            bind_mounts: vec![],
            user: None,
            detach: true,
            remove_on_exit: false,
            workdir: None,
            command: vec![],
        }
    }

    fn short_probe_use_case(port: Arc<dyn ContainerPort>) -> EnsureContainerUseCase {
        EnsureContainerUseCase::with_probe_settings(
            port,
            StartupProbeSettings {
                timeout: Duration::from_millis(30),
                poll_interval: Duration::from_millis(5),
                log_tail_lines: 10,
            },
        )
    }

    #[tokio::test]
    async fn pulls_missing_image_before_running_container() {
        let port = Arc::new(MockContainerPort::new(MockState::default()));
        let use_case = short_probe_use_case(port.clone());
        let config = sample_config();

        let (id, _) = use_case.ensure(&config).await.unwrap();

        assert_eq!(id.0, config.name);

        let state = port.snapshot();
        assert_eq!(state.pull_count, 1);
        assert_eq!(state.run_count, 1);
        assert_eq!(state.stop_count, 0);
        assert_eq!(state.remove_count, 0);
        assert_eq!(
            state.actions,
            vec![
                "image_exists",
                "pull_image",
                "image_exists",
                "exists",
                "run",
                "is_running"
            ]
        );
    }

    #[tokio::test]
    async fn restarts_existing_running_container_without_repulling_existing_image() {
        let port = Arc::new(MockContainerPort::new(MockState {
            image_exists: true,
            container_exists: true,
            container_running: true,
            ..Default::default()
        }));
        let use_case = short_probe_use_case(port.clone());
        let config = sample_config();

        let (id, _) = use_case.ensure(&config).await.unwrap();

        assert_eq!(id.0, config.name);

        let state = port.snapshot();
        assert_eq!(state.pull_count, 0);
        assert_eq!(state.run_count, 1);
        assert_eq!(state.stop_count, 1);
        assert_eq!(state.remove_count, 1);
        assert_eq!(
            state.actions,
            vec![
                "image_exists",
                "exists",
                "is_running",
                "stop",
                "remove",
                "run",
                "is_running"
            ]
        );
    }

    #[tokio::test]
    async fn removes_existing_stopped_container_before_fresh_start() {
        let port = Arc::new(MockContainerPort::new(MockState {
            image_exists: true,
            container_exists: true,
            container_running: false,
            ..Default::default()
        }));
        let use_case = short_probe_use_case(port.clone());
        let config = sample_config();

        let (id, _) = use_case.ensure(&config).await.unwrap();

        assert_eq!(id.0, config.name);

        let state = port.snapshot();
        assert_eq!(state.pull_count, 0);
        assert_eq!(state.run_count, 1);
        assert_eq!(state.stop_count, 0);
        assert_eq!(state.remove_count, 1);
        assert_eq!(
            state.actions,
            vec![
                "image_exists",
                "exists",
                "is_running",
                "remove",
                "run",
                "is_running"
            ]
        );
    }

    #[tokio::test]
    async fn returns_error_when_container_exits_during_startup_probe() {
        let port = Arc::new(MockContainerPort::new(MockState {
            image_exists: true,
            running_checks: vec![false],
            ..Default::default()
        }));
        let use_case = short_probe_use_case(port.clone());
        let config = sample_config();

        let error = use_case
            .ensure(&config)
            .await
            .expect_err("container should fail startup verification");

        match error {
            ContainerError::StartupFailed { summary, .. } => {
                assert!(summary.contains("exited during startup verification"));
            }
            other => panic!("expected startup failure, got {other:?}"),
        }

        let state = port.snapshot();
        assert_eq!(state.logs_tail_count, 1);
        assert!(state.actions.ends_with(&["run", "is_running", "logs_tail"]));
    }

    #[tokio::test]
    async fn startup_failure_includes_recent_container_logs() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let unused_port = listener.local_addr().unwrap().port();
        drop(listener);

        let port = Arc::new(MockContainerPort::new(MockState {
            image_exists: true,
            logs_tail_output: Some(
                "Vault path does not exist: /Obsidian/MyVault\ncontainer exiting".into(),
            ),
            ..Default::default()
        }));
        let use_case = short_probe_use_case(port.clone());
        let mut config = sample_config();
        config.port_mappings = vec![PortMapping {
            host_address: "127.0.0.1".into(),
            host_port: unused_port,
            container_port: 2765,
        }];

        let error = use_case
            .ensure(&config)
            .await
            .expect_err("container should fail readiness timeout");

        match error {
            ContainerError::StartupFailed {
                summary,
                logs: Some(logs),
            } => {
                assert!(summary.contains("did not become reachable"));
                assert!(logs.contains("Vault path does not exist"));
            }
            other => panic!("expected startup failure with logs, got {other:?}"),
        }

        let state = port.snapshot();
        assert_eq!(state.logs_tail_count, 1);
        assert!(state.actions.contains(&"logs_tail"));
    }

    #[tokio::test]
    async fn aborts_startup_when_network_isolation_setup_fails() {
        let port = Arc::new(MockContainerPort::new(MockState {
            image_exists: true,
            prepare_network_isolation_result: false,
            ..Default::default()
        }));
        let use_case = short_probe_use_case(port.clone());
        let mut config = sample_config();
        config.isolation_strategy = Some(ContainerNetworkIsolationStrategy::DiscoverContainerIp);

        let err = use_case.ensure(&config).await.unwrap_err();
        assert!(
            matches!(err, ContainerError::Other(_)),
            "expected ContainerError::Other, got {err:?}"
        );

        let state = port.snapshot();
        assert_eq!(state.prepare_network_isolation_count, 1);
        assert_eq!(
            state.actions,
            vec!["image_exists", "exists", "prepare_network_isolation"]
        );
    }
}
