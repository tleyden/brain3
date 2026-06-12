use std::sync::Arc;

use crate::domain::errors::SetupError;
use crate::domain::model::ContainerRuntime;
use crate::domain::setup::{
    ConnectionCard, FinalizeSetupRequest, SetupDefaults, SetupDraftConfig, SetupPreparation,
    SetupSummary, TunnelModeDraft, DEFAULT_ACCESS_TOKEN_LIFETIME_SECS, DEFAULT_CLIENT_ID,
    DEFAULT_CONTAINER_HOST_PORT, DEFAULT_CONTAINER_MCP_PORT, DEFAULT_GATEWAY_PORT,
    DEFAULT_GENERATED_PASSWORD_LENGTH, DEFAULT_GENERATED_SECRET_BYTES,
    DEFAULT_REFRESH_TOKEN_LIFETIME_SECS, DEFAULT_USERNAME,
};
use crate::ports::setup_system::SetupSystemPort;

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
            tunnel_mode: TunnelModeDraft::CloudflareQuick,
            container_runtime: default_container_runtime(self.port.operating_system()),
            vault_path: std::path::PathBuf::new(),
            container_image: self.defaults.default_container_image.clone(),
            container_host_port: DEFAULT_CONTAINER_HOST_PORT,
            container_mcp_port: DEFAULT_CONTAINER_MCP_PORT,
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use crate::domain::errors::SetupError;
    use crate::domain::model::ContainerRuntime;
    use crate::domain::setup::{
        ConnectionCard, DependencyAvailability, DependencyStatus, FinalizeSetupRequest,
        PackageManager, SetupDefaults, SetupDraftConfig, SetupOperatingSystem, SetupPaths,
        TunnelModeDraft,
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

        async fn create_temp_log_file(&self) -> Result<PathBuf, SetupError> {
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
            gateway_port: 8421,
            client_id: "brain3-oauth2-client".into(),
            client_secret: String::new(),
            access_token_lifetime_secs: DEFAULT_ACCESS_TOKEN_LIFETIME_SECS,
            refresh_token_lifetime_secs: DEFAULT_REFRESH_TOKEN_LIFETIME_SECS,
            username: "admin".into(),
            password: String::new(),
            tunnel_mode: TunnelModeDraft::CloudflareQuick,
            container_runtime: ContainerRuntime::MacOSContainer,
            vault_path,
            container_image: "ghcr.io/tleyden/brain3-mcp-vault-tools:latest".into(),
            container_host_port: 8420,
            container_mcp_port: 8420,
            pkce_required: true,
            enforce_hostname_check: true,
            direct_public_origin_hostname: None,
        }
    }

    fn sample_defaults() -> SetupDefaults {
        SetupDefaults {
            default_container_image: "ghcr.io/tleyden/brain3-mcp-vault-tools:v0.1.5".into(),
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
    }

    #[tokio::test]
    async fn prepare_uses_injected_default_container_image() {
        let port = Arc::new(MockSetupSystemPort::new(vec![]));
        let use_case = FirstRunSetupUseCase::new(
            port,
            SetupDefaults {
                default_container_image: "ghcr.io/tleyden/brain3-mcp-vault-tools:v9.9.9".into(),
            },
        );

        let preparation = use_case.prepare().await.expect("prepare should succeed");

        assert_eq!(
            preparation.draft.container_image,
            "ghcr.io/tleyden/brain3-mcp-vault-tools:v9.9.9"
        );
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
}
