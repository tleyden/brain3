use std::path::{Path, PathBuf};

use crate::domain::errors::SetupError;
use crate::domain::setup::{
    DependencyStatus, InstallAction, SetupDraftConfig, SetupOperatingSystem, SetupPaths,
};

#[async_trait::async_trait]
pub trait SetupSystemPort: Send + Sync {
    fn operating_system(&self) -> SetupOperatingSystem;

    fn resolve_paths(&self) -> Result<SetupPaths, SetupError>;

    async fn collect_dependency_status(&self) -> Result<DependencyStatus, SetupError>;

    fn generate_secret_hex(&self, num_bytes: usize) -> Result<String, SetupError>;

    fn generate_password(&self, length: usize) -> Result<String, SetupError>;

    fn render_env_file(
        &self,
        draft: &SetupDraftConfig,
        paths: &SetupPaths,
    ) -> Result<String, SetupError>;

    async fn ensure_app_home_dirs(&self, paths: &SetupPaths) -> Result<(), SetupError>;

    async fn write_env_file(&self, path: &Path, contents: &str) -> Result<(), SetupError>;

    async fn path_exists(&self, path: &Path) -> Result<bool, SetupError>;

    async fn create_temp_log_file(&self) -> Result<PathBuf, SetupError>;

    async fn run_install_action(&self, action: InstallAction) -> Result<(), SetupError>;
}
