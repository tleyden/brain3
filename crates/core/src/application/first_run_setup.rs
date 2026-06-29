use std::sync::Arc;

use crate::domain::errors::SetupError;
use crate::domain::model::{AccessMode, ContainerRuntime, GatewayConfig, TunnelConfig};
use crate::domain::setup::{
    AccessModeDraft, ConnectionCard, FinalizeSetupRequest, SetupDefaults, SetupDraftConfig,
    SetupPreparation, SetupSummary, TunnelModeDraft, DEFAULT_ACCESS_TOKEN_LIFETIME_SECS,
    DEFAULT_CLIENT_ID, DEFAULT_CONTAINER_HOST_PORT, DEFAULT_CONTAINER_MCP_PORT,
    DEFAULT_CONTAINER_NAME, DEFAULT_CONTAINER_NETWORK_NAME, DEFAULT_GATEWAY_PORT,
    DEFAULT_GENERATED_PASSWORD_LENGTH, DEFAULT_GENERATED_SECRET_BYTES, DEFAULT_LOCAL_MCP_PORT,
    DEFAULT_REFRESH_TOKEN_LIFETIME_SECS, DEFAULT_USERNAME,
};
use crate::ports::setup_system::SetupSystemPort;

pub const CURRENT_RELEASE: &str = "v0.2.7";

pub struct FirstRunSetupUseCase {
    port: Arc<dyn SetupSystemPort>,
    defaults: SetupDefaults,
}

impl FirstRunSetupUseCase {
    pub fn new(port: Arc<dyn SetupSystemPort>, defaults: SetupDefaults) -> Self {
        Self { port, defaults }
    }

    pub async fn prepare(&self) -> Result<SetupPreparation, SetupError> {
        let paths = self.port.resolve_paths()?;
        let dependencies = self.port.collect_dependency_status().await?;

        let draft = SetupDraftConfig {
            gateway_port: DEFAULT_GATEWAY_PORT,
            client_id: DEFAULT_CLIENT_ID.to_string(),
            client_secret: self
                .port
                .generate_secret_hex(DEFAULT_GENERATED_SECRET_BYTES)?,
            access_token_lifetime_secs: DEFAULT_ACCESS_TOKEN_LIFETIME_SECS,
            refresh_token_lifetime_secs: DEFAULT_REFRESH_TOKEN_LIFETIME_SECS,
            username: DEFAULT_USERNAME.to_string(),
            password: String::new(),
            access_mode: AccessModeDraft::Both,
            tunnel_mode: TunnelModeDraft::CloudflareQuick,
            container_runtime: default_container_runtime(self.port.operating_system()),
            vault_path: std::path::PathBuf::new(),
            container_image_repo: self.defaults.default_container_image_repo.clone(),
            container_host_port: DEFAULT_CONTAINER_HOST_PORT,
            container_mcp_port: DEFAULT_CONTAINER_MCP_PORT,
            container_name: DEFAULT_CONTAINER_NAME.to_string(),
            container_network_isolated: true,
            container_network_name: DEFAULT_CONTAINER_NETWORK_NAME.to_string(),
            local_mcp_enabled: true,
            local_mcp_port: DEFAULT_LOCAL_MCP_PORT,
            local_mcp_bearer_token: self
                .port
                .generate_secret_hex(DEFAULT_GENERATED_SECRET_BYTES)?,
            pkce_required: true,
            enforce_hostname_check: true,
            direct_public_origin_hostname: None,
        };

        Ok(SetupPreparation {
            paths,
            draft,
            dependencies,
        })
    }

    pub async fn prepare_from_existing_config(
        &self,
        config: &GatewayConfig,
    ) -> Result<SetupPreparation, SetupError> {
        let paths = self.port.resolve_paths()?;
        let dependencies = self.port.collect_dependency_status().await?;

        Ok(SetupPreparation {
            paths,
            draft: self.draft_from_existing_config(config),
            dependencies,
        })
    }

    pub async fn validate_vault_path(
        &self,
        vault_path: &std::path::Path,
    ) -> Result<(), SetupError> {
        if !vault_path.is_absolute() {
            return Err(SetupError::Invalid(
                "vault path must be an absolute path".into(),
            ));
        }
        if !self.port.path_exists(vault_path).await? {
            return Err(SetupError::Invalid(format!(
                "vault path does not exist: {}",
                vault_path.display()
            )));
        }

        Ok(())
    }

    pub async fn finalize(
        &self,
        request: FinalizeSetupRequest,
    ) -> Result<SetupSummary, SetupError> {
        let paths = self.port.resolve_paths()?;
        let dependencies = self.port.collect_dependency_status().await?;
        let mut draft = request.draft;

        validate_nonempty("client ID", &draft.client_id)?;
        validate_nonempty("username", &draft.username)?;
        validate_positive_u64("access token lifetime", draft.access_token_lifetime_secs)?;
        validate_positive_u64("refresh token lifetime", draft.refresh_token_lifetime_secs)?;
        self.validate_vault_path(&draft.vault_path).await?;

        if draft.client_secret.trim().is_empty() {
            draft.client_secret = self
                .port
                .generate_secret_hex(DEFAULT_GENERATED_SECRET_BYTES)?;
        }

        if request.generate_password {
            draft.password = self
                .port
                .generate_password(DEFAULT_GENERATED_PASSWORD_LENGTH)?;
        } else {
            validate_nonempty("password", &draft.password)?;
        }

        apply_access_mode_policy(&mut draft);

        if draft.local_mcp_enabled && draft.local_mcp_bearer_token.trim().is_empty() {
            draft.local_mcp_bearer_token = self
                .port
                .generate_secret_hex(DEFAULT_GENERATED_SECRET_BYTES)?;
        }

        let env_contents = self.port.render_env_file(&draft, &paths)?;
        self.port.ensure_app_home_dirs(&paths).await?;
        self.port
            .write_env_file(&paths.env_file, &env_contents)
            .await?;

        Ok(SetupSummary {
            paths,
            draft,
            dependencies,
        })
    }

    pub fn build_connection_card(
        &self,
        server_url: impl Into<String>,
        log_file: std::path::PathBuf,
        summary: &SetupSummary,
    ) -> ConnectionCard {
        ConnectionCard {
            server_url: server_url.into(),
            client_id: summary.draft.client_id.clone(),
            client_secret: summary.draft.client_secret.clone(),
            username: summary.draft.username.clone(),
            password: summary.draft.password.clone(),
            log_file,
        }
    }

    fn draft_from_existing_config(&self, config: &GatewayConfig) -> SetupDraftConfig {
        let container = config.container.as_ref();
        let local_mcp = config.local_mcp.as_ref();

        SetupDraftConfig {
            gateway_port: config.port,
            client_id: config.oauth.client_id.clone(),
            client_secret: config.oauth.client_secret.clone(),
            access_token_lifetime_secs: config.oauth.access_token_lifetime_secs,
            refresh_token_lifetime_secs: config.oauth.refresh_token_lifetime_secs,
            username: config.oauth.username.clone(),
            password: config.oauth.password.clone(),
            access_mode: access_mode_draft_from_config(config.access_mode),
            tunnel_mode: tunnel_mode_draft_from_config(config.tunnel.as_ref()),
            container_runtime: container
                .map(|container| container.runtime)
                .unwrap_or_else(|| default_container_runtime(self.port.operating_system())),
            vault_path: container
                .map(|container| container.vault_path.clone())
                .unwrap_or_default(),
            container_image_repo: container
                .map(|container| image_repo_from_reference(&container.image))
                .unwrap_or_else(|| self.defaults.default_container_image_repo.clone()),
            container_host_port: container
                .map(|container| container.host_port)
                .unwrap_or(DEFAULT_CONTAINER_HOST_PORT),
            container_mcp_port: container
                .map(|container| container.container_port)
                .unwrap_or(DEFAULT_CONTAINER_MCP_PORT),
            container_name: container
                .map(|container| container.container_name.clone())
                .unwrap_or_else(|| DEFAULT_CONTAINER_NAME.to_string()),
            container_network_isolated: container
                .and_then(|container| container.isolation_strategy)
                .is_some(),
            container_network_name: container
                .map(|container| container.network_name.clone())
                .unwrap_or_else(|| DEFAULT_CONTAINER_NETWORK_NAME.to_string()),
            local_mcp_enabled: local_mcp.is_some(),
            local_mcp_port: local_mcp
                .map(|local_mcp| local_mcp.port)
                .unwrap_or(DEFAULT_LOCAL_MCP_PORT),
            local_mcp_bearer_token: local_mcp
                .map(|local_mcp| local_mcp.bearer_token.clone())
                .unwrap_or_default(),
            pkce_required: config.oauth.pkce_required,
            enforce_hostname_check: config.hostname_validation.enforce,
            direct_public_origin_hostname: None,
        }
    }
}

fn default_container_runtime(
    operating_system: crate::domain::setup::SetupOperatingSystem,
) -> ContainerRuntime {
    match operating_system {
        crate::domain::setup::SetupOperatingSystem::MacOS => ContainerRuntime::MacOSContainer,
        crate::domain::setup::SetupOperatingSystem::Linux => ContainerRuntime::Docker,
    }
}

fn validate_nonempty(label: &str, value: &str) -> Result<(), SetupError> {
    if value.trim().is_empty() {
        return Err(SetupError::Invalid(format!("{label} must not be empty")));
    }
    Ok(())
}

fn validate_positive_u64(label: &str, value: u64) -> Result<(), SetupError> {
    if value == 0 {
        return Err(SetupError::Invalid(format!(
            "{label} must be greater than 0"
        )));
    }
    Ok(())
}

fn apply_access_mode_policy(draft: &mut SetupDraftConfig) {
    match draft.access_mode {
        AccessModeDraft::LocalOnly => {
            draft.local_mcp_enabled = true;
            draft.tunnel_mode = TunnelModeDraft::Disabled;
        }
        AccessModeDraft::RemoteOnly => {
            draft.local_mcp_enabled = false;
            if matches!(draft.tunnel_mode, TunnelModeDraft::Disabled) {
                draft.tunnel_mode = TunnelModeDraft::CloudflareQuick;
            }
        }
        AccessModeDraft::Both => {
            draft.local_mcp_enabled = true;
            if matches!(draft.tunnel_mode, TunnelModeDraft::Disabled) {
                draft.tunnel_mode = TunnelModeDraft::CloudflareQuick;
            }
        }
    }
}

fn access_mode_draft_from_config(access_mode: AccessMode) -> AccessModeDraft {
    match access_mode {
        AccessMode::Local => AccessModeDraft::LocalOnly,
        AccessMode::Remote => AccessModeDraft::RemoteOnly,
        AccessMode::Both => AccessModeDraft::Both,
    }
}

fn tunnel_mode_draft_from_config(tunnel: Option<&TunnelConfig>) -> TunnelModeDraft {
    match tunnel {
        Some(TunnelConfig::CloudflareQuick { .. }) => TunnelModeDraft::CloudflareQuick,
        Some(TunnelConfig::CloudflareNamed {
            tunnel_name,
            domain,
            ..
        }) => TunnelModeDraft::CloudflareNamed {
            tunnel_name: tunnel_name.clone(),
            domain: domain.clone(),
        },
        None => TunnelModeDraft::Disabled,
    }
}

fn image_repo_from_reference(image: &str) -> String {
    let trimmed = image.trim();
    let without_digest = trimmed.split('@').next().unwrap_or(trimmed);

    match without_digest.rsplit_once(':') {
        Some((repo, tag_or_port))
            if !tag_or_port.contains('/') && repo.rsplit('/').next().is_some() =>
        {
            repo.to_string()
        }
        _ => without_digest.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use crate::domain::errors::SetupError;
    use crate::domain::model::ContainerRuntime;
    use crate::domain::setup::{
        AccessModeDraft, ConnectionCard, DependencyAvailability, DependencyStatus,
        FinalizeSetupRequest, PackageManager, SetupDefaults, SetupDraftConfig,
        SetupOperatingSystem, SetupPaths, TunnelModeDraft,
    };
    use crate::ports::setup_system::SetupSystemPort;

    use super::*;

    #[derive(Default, Clone)]
    struct MockState {
        rendered_env: Option<String>,
        written_env: Option<String>,
        generated_secret_count: usize,
        generated_password_count: usize,
    }

    struct MockSetupSystemPort {
        state: Mutex<MockState>,
        paths: SetupPaths,
        dependencies: DependencyStatus,
        existing_vault_paths: Vec<PathBuf>,
    }

    impl MockSetupSystemPort {
        fn new(existing_vault_paths: Vec<PathBuf>) -> Self {
            Self {
                state: Mutex::new(MockState::default()),
                paths: SetupPaths::new(
                    PathBuf::from("/tmp/brain3-home"),
                    PathBuf::from("/tmp/brain3-home/.env"),
                    PathBuf::from("/tmp/brain3-home/cloudflared"),
                ),
                dependencies: DependencyStatus {
                    operating_system: SetupOperatingSystem::MacOS,
                    package_manager: Some(PackageManager::Homebrew),
                    cloudflared: DependencyAvailability::Installed,
                    preferred_container_runtime: DependencyAvailability::Installed,
                    docker_installed: false,
                    macos_container_installed: Some(true),
                    homebrew_installed: Some(true),
                },
                existing_vault_paths,
            }
        }

        fn snapshot(&self) -> MockState {
            self.state.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl SetupSystemPort for MockSetupSystemPort {
        fn operating_system(&self) -> SetupOperatingSystem {
            SetupOperatingSystem::MacOS
        }

        fn resolve_paths(&self) -> Result<SetupPaths, SetupError> {
            Ok(self.paths.clone())
        }

        async fn collect_dependency_status(&self) -> Result<DependencyStatus, SetupError> {
            Ok(self.dependencies.clone())
        }

        fn generate_secret_hex(&self, _num_bytes: usize) -> Result<String, SetupError> {
            let mut state = self.state.lock().unwrap();
            state.generated_secret_count += 1;
            Ok(format!("generated-secret-{}", state.generated_secret_count))
        }

        fn generate_password(&self, _length: usize) -> Result<String, SetupError> {
            let mut state = self.state.lock().unwrap();
            state.generated_password_count += 1;
            Ok(format!(
                "generated-password-{}",
                state.generated_password_count
            ))
        }

        fn render_env_file(
            &self,
            draft: &SetupDraftConfig,
            _paths: &SetupPaths,
        ) -> Result<String, SetupError> {
            let rendered = format!(
                "USERNAME={}\nCLIENT_ID={}\nSECRET={}\nPASSWORD={}\n",
                draft.username, draft.client_id, draft.client_secret, draft.password
            );
            self.state.lock().unwrap().rendered_env = Some(rendered.clone());
            Ok(rendered)
        }

        async fn ensure_app_home_dirs(&self, _paths: &SetupPaths) -> Result<(), SetupError> {
            Ok(())
        }

        async fn write_env_file(&self, _path: &Path, contents: &str) -> Result<(), SetupError> {
            self.state.lock().unwrap().written_env = Some(contents.to_string());
            Ok(())
        }

        async fn path_exists(&self, path: &Path) -> Result<bool, SetupError> {
            Ok(self
                .existing_vault_paths
                .iter()
                .any(|candidate| candidate == path))
        }

        async fn resolve_log_file(&self, _paths: &SetupPaths) -> Result<PathBuf, SetupError> {
            Ok(PathBuf::from("/tmp/brain3.log"))
        }

        async fn run_install_action(
            &self,
            _action: crate::domain::setup::InstallAction,
        ) -> Result<(), SetupError> {
            Ok(())
        }
    }

    fn sample_draft(vault_path: PathBuf) -> SetupDraftConfig {
        SetupDraftConfig {
            gateway_port: 2763,
            client_id: "brain3-oauth2-client".into(),
            client_secret: String::new(),
            access_token_lifetime_secs: DEFAULT_ACCESS_TOKEN_LIFETIME_SECS,
            refresh_token_lifetime_secs: DEFAULT_REFRESH_TOKEN_LIFETIME_SECS,
            username: "admin".into(),
            password: String::new(),
            access_mode: AccessModeDraft::Both,
            tunnel_mode: TunnelModeDraft::CloudflareQuick,
            container_runtime: ContainerRuntime::MacOSContainer,
            vault_path,
            container_image_repo: "ghcr.io/tleyden/brain3-mcp-vault-tools".into(),
            container_host_port: 2765,
            container_mcp_port: 2765,
            container_name: DEFAULT_CONTAINER_NAME.into(),
            container_network_isolated: false,
            container_network_name: DEFAULT_CONTAINER_NETWORK_NAME.into(),
            local_mcp_enabled: true,
            local_mcp_port: DEFAULT_LOCAL_MCP_PORT,
            local_mcp_bearer_token: "local-secret".into(),
            pkce_required: true,
            enforce_hostname_check: true,
            direct_public_origin_hostname: None,
        }
    }

    fn sample_defaults() -> SetupDefaults {
        SetupDefaults {
            default_container_image_repo: "ghcr.io/tleyden/brain3-mcp-vault-tools".into(),
        }
    }

    #[tokio::test]
    async fn finalize_rejects_relative_vault_paths() {
        let port = Arc::new(MockSetupSystemPort::new(vec![]));
        let use_case = FirstRunSetupUseCase::new(port, sample_defaults());

        let error = use_case
            .finalize(FinalizeSetupRequest {
                draft: sample_draft(PathBuf::from("relative/vault")),
                generate_password: true,
            })
            .await
            .expect_err("expected relative path to be rejected");

        match error {
            SetupError::Invalid(message) => assert!(message.contains("absolute")),
            other => panic!("expected invalid setup error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_vault_path_rejects_missing_absolute_path() {
        let port = Arc::new(MockSetupSystemPort::new(vec![]));
        let use_case = FirstRunSetupUseCase::new(port, sample_defaults());

        let error = use_case
            .validate_vault_path(Path::new("/Users/test/missing-vault"))
            .await
            .expect_err("expected missing path to be rejected");

        match error {
            SetupError::Invalid(message) => assert!(message.contains("does not exist")),
            other => panic!("expected invalid setup error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_vault_path_accepts_existing_absolute_path() {
        let vault_path = PathBuf::from("/Users/test/vault");
        let port = Arc::new(MockSetupSystemPort::new(vec![vault_path.clone()]));
        let use_case = FirstRunSetupUseCase::new(port, sample_defaults());

        use_case
            .validate_vault_path(&vault_path)
            .await
            .expect("existing absolute path should be accepted");
    }

    #[tokio::test]
    async fn prepare_uses_brain3_oauth2_client_as_default_client_id() {
        let port = Arc::new(MockSetupSystemPort::new(vec![]));
        let use_case = FirstRunSetupUseCase::new(port, sample_defaults());

        let preparation = use_case.prepare().await.expect("prepare should succeed");

        assert_eq!(preparation.draft.client_id, "brain3-oauth2-client");
        assert_eq!(preparation.draft.access_mode, AccessModeDraft::Both);
        assert!(preparation.draft.container_network_isolated);
        assert!(preparation.draft.local_mcp_enabled);
        assert!(!preparation.draft.local_mcp_bearer_token.is_empty());
    }

    #[tokio::test]
    async fn prepare_uses_injected_default_container_image_repo() {
        let port = Arc::new(MockSetupSystemPort::new(vec![]));
        let use_case = FirstRunSetupUseCase::new(
            port,
            SetupDefaults {
                default_container_image_repo: "ghcr.io/tleyden/brain3-mcp-vault-tools".into(),
            },
        );

        let preparation = use_case.prepare().await.expect("prepare should succeed");

        assert_eq!(
            preparation.draft.container_image_repo,
            "ghcr.io/tleyden/brain3-mcp-vault-tools"
        );
    }

    #[tokio::test]
    async fn prepare_sets_default_container_names() {
        let port = Arc::new(MockSetupSystemPort::new(vec![]));
        let use_case = FirstRunSetupUseCase::new(port, sample_defaults());

        let preparation = use_case.prepare().await.expect("prepare should succeed");

        assert_eq!(preparation.draft.container_name, DEFAULT_CONTAINER_NAME);
        assert_eq!(
            preparation.draft.container_network_name,
            DEFAULT_CONTAINER_NETWORK_NAME
        );
    }

    #[tokio::test]
    async fn prepare_from_existing_config_uses_saved_container_identity_and_password() {
        let port = Arc::new(MockSetupSystemPort::new(vec![]));
        let use_case = FirstRunSetupUseCase::new(port, sample_defaults());

        let preparation = use_case
            .prepare_from_existing_config(&GatewayConfig {
                port: 9421,
                host: "127.0.0.1".into(),
                token_db_path: PathBuf::from("/tmp/brain3-home/brain3.db"),
                oauth: crate::domain::model::OAuthConfig {
                    client_id: "saved-client".into(),
                    client_secret: "saved-secret".into(),
                    access_token_lifetime_secs: 1234,
                    refresh_token_lifetime_secs: 5678,
                    pkce_required: false,
                    username: "saved-user".into(),
                    password: "saved-password".into(),
                },
                mcp_reverse_proxy: crate::domain::model::MCPReverseProxyConfig {
                    mcp_upstream_url: "http://127.0.0.1:2765".into(),
                    upstream_secret: "upstream-secret".into(),
                },
                hostname_validation: crate::domain::model::HostnameValidationConfig {
                    expected_host: None,
                    enforce: false,
                },
                access_mode: crate::domain::model::AccessMode::Both,
                local_mcp: Some(crate::domain::model::LocalMcpConfig {
                    port: 9555,
                    bearer_token: "local-bearer".into(),
                }),
                container: Some(crate::domain::model::ContainerStartupConfig {
                    runtime: ContainerRuntime::Docker,
                    image: "ghcr.io/example/custom-mcp:v9.9.9".into(),
                    container_name: "saved-container".into(),
                    network_name: "saved-network".into(),
                    vault_path: PathBuf::from("/srv/vault"),
                    upstream_secret: "upstream-secret".into(),
                    host_port: 9556,
                    container_port: 9557,
                    isolation_strategy: Some(
                        crate::domain::model::ContainerNetworkIsolationStrategy::DiscoverContainerIp,
                    ),
                    dev_mount_source: None,
                    mcp_log_level: None,
                }),
                tunnel: Some(crate::domain::model::TunnelConfig::CloudflareQuick {
                    local_port: 9421,
                }),
            })
            .await
            .expect("prepare_from_existing_config should succeed");

        assert_eq!(preparation.draft.client_id, "saved-client");
        assert_eq!(preparation.draft.client_secret, "saved-secret");
        assert_eq!(preparation.draft.username, "saved-user");
        assert_eq!(preparation.draft.password, "saved-password");
        assert_eq!(preparation.draft.container_name, "saved-container");
        assert_eq!(preparation.draft.container_network_name, "saved-network");
        assert_eq!(
            preparation.draft.container_image_repo,
            "ghcr.io/example/custom-mcp"
        );
        assert_eq!(preparation.draft.container_host_port, 9556);
        assert_eq!(preparation.draft.container_mcp_port, 9557);
        assert_eq!(preparation.draft.local_mcp_port, 9555);
        assert_eq!(preparation.draft.local_mcp_bearer_token, "local-bearer");
        assert!(!preparation.draft.pkce_required);
        assert!(!preparation.draft.enforce_hostname_check);
    }

    #[tokio::test]
    async fn finalize_generates_missing_secrets_and_password() {
        let vault_path = PathBuf::from("/Users/test/vault");
        let port = Arc::new(MockSetupSystemPort::new(vec![vault_path.clone()]));
        let use_case = FirstRunSetupUseCase::new(port.clone(), sample_defaults());

        let result = use_case
            .finalize(FinalizeSetupRequest {
                draft: sample_draft(vault_path),
                generate_password: true,
            })
            .await
            .expect("finalize should succeed");

        assert_eq!(result.draft.client_secret, "generated-secret-1");
        assert_eq!(result.draft.password, "generated-password-1");
        assert_eq!(port.snapshot().generated_secret_count, 1);
        assert_eq!(port.snapshot().generated_password_count, 1);
    }

    #[tokio::test]
    async fn finalize_writes_env_and_builds_connection_card() {
        let vault_path = PathBuf::from("/Users/test/vault");
        let port = Arc::new(MockSetupSystemPort::new(vec![vault_path.clone()]));
        let use_case = FirstRunSetupUseCase::new(port.clone(), sample_defaults());

        let summary = use_case
            .finalize(FinalizeSetupRequest {
                draft: SetupDraftConfig {
                    password: "chosen-password".into(),
                    client_secret: "chosen-secret".into(),
                    ..sample_draft(vault_path)
                },
                generate_password: false,
            })
            .await
            .expect("finalize should succeed");

        let card = use_case.build_connection_card(
            "https://example.trycloudflare.com",
            PathBuf::from("/tmp/brain3.log"),
            &summary,
        );
        let snapshot = port.snapshot();

        assert!(snapshot
            .written_env
            .expect("env should have been written")
            .contains("CLIENT_ID=brain3-oauth2-client"));
        assert_eq!(
            card,
            ConnectionCard {
                server_url: "https://example.trycloudflare.com".into(),
                client_id: "brain3-oauth2-client".into(),
                client_secret: "chosen-secret".into(),
                username: "admin".into(),
                password: "chosen-password".into(),
                log_file: PathBuf::from("/tmp/brain3.log"),
            }
        );
    }

    #[tokio::test]
    async fn finalize_local_only_forces_local_mcp_and_disables_tunnel() {
        let vault_path = PathBuf::from("/Users/test/vault");
        let port = Arc::new(MockSetupSystemPort::new(vec![vault_path.clone()]));
        let use_case = FirstRunSetupUseCase::new(port.clone(), sample_defaults());

        let summary = use_case
            .finalize(FinalizeSetupRequest {
                draft: SetupDraftConfig {
                    access_mode: AccessModeDraft::LocalOnly,
                    local_mcp_enabled: false,
                    local_mcp_bearer_token: String::new(),
                    tunnel_mode: TunnelModeDraft::CloudflareQuick,
                    password: "chosen-password".into(),
                    client_secret: "chosen-secret".into(),
                    ..sample_draft(vault_path)
                },
                generate_password: false,
            })
            .await
            .expect("finalize should succeed");

        assert_eq!(summary.draft.access_mode, AccessModeDraft::LocalOnly);
        assert!(summary.draft.local_mcp_enabled);
        assert_eq!(summary.draft.tunnel_mode, TunnelModeDraft::Disabled);
        assert_eq!(summary.draft.local_mcp_bearer_token, "generated-secret-1");
        assert_eq!(port.snapshot().generated_secret_count, 1);
    }

    #[tokio::test]
    async fn finalize_remote_only_disables_local_mcp_and_restores_quick_tunnel() {
        let vault_path = PathBuf::from("/Users/test/vault");
        let port = Arc::new(MockSetupSystemPort::new(vec![vault_path.clone()]));
        let use_case = FirstRunSetupUseCase::new(port, sample_defaults());

        let summary = use_case
            .finalize(FinalizeSetupRequest {
                draft: SetupDraftConfig {
                    access_mode: AccessModeDraft::RemoteOnly,
                    local_mcp_enabled: true,
                    tunnel_mode: TunnelModeDraft::Disabled,
                    password: "chosen-password".into(),
                    client_secret: "chosen-secret".into(),
                    ..sample_draft(vault_path)
                },
                generate_password: false,
            })
            .await
            .expect("finalize should succeed");

        assert_eq!(summary.draft.access_mode, AccessModeDraft::RemoteOnly);
        assert!(!summary.draft.local_mcp_enabled);
        assert_eq!(summary.draft.tunnel_mode, TunnelModeDraft::CloudflareQuick);
    }
}
