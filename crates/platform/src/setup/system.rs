use std::env;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use brain3_core::domain::errors::SetupError;
use brain3_core::domain::setup::{
    DependencyStatus, InstallAction, PackageManager, SetupDraftConfig, SetupOperatingSystem,
    SetupPaths,
};
use brain3_core::ports::setup_system::SetupSystemPort;
use rand::Rng;
use tokio::fs;

use super::app_home::Brain3AppHome;
use super::env_writer::render_env_file;

#[derive(Debug, Default, Clone, Copy)]
pub struct PlatformSetupSystem;

impl PlatformSetupSystem {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SetupSystemPort for PlatformSetupSystem {
    fn operating_system(&self) -> SetupOperatingSystem {
        operating_system()
    }

    fn resolve_paths(&self) -> Result<SetupPaths, SetupError> {
        Ok(Brain3AppHome::resolve_from_env()?.as_setup_paths())
    }

    async fn collect_dependency_status(&self) -> Result<DependencyStatus, SetupError> {
        let operating_system = operating_system();
        let homebrew_installed = match operating_system {
            SetupOperatingSystem::MacOS => Some(binary_on_path("brew")),
            SetupOperatingSystem::Linux => None,
        };
        let package_manager = match operating_system {
            SetupOperatingSystem::MacOS if homebrew_installed == Some(true) => {
                Some(PackageManager::Homebrew)
            }
            SetupOperatingSystem::Linux if binary_on_path("apt-get") => Some(PackageManager::Apt),
            _ => None,
        };
        let macos_container_installed = match operating_system {
            SetupOperatingSystem::MacOS => Some(binary_on_path("container")),
            SetupOperatingSystem::Linux => None,
        };

        Ok(DependencyStatus {
            operating_system,
            package_manager,
            cloudflared_installed: binary_on_path("cloudflared"),
            docker_installed: binary_on_path("docker"),
            macos_container_installed,
            homebrew_installed,
        })
    }

    fn generate_secret_hex(&self, num_bytes: usize) -> Result<String, SetupError> {
        use rand::RngCore;

        let mut bytes = vec![0u8; num_bytes];
        rand::rng().fill_bytes(&mut bytes);
        let mut output = String::with_capacity(num_bytes.saturating_mul(2));
        for byte in bytes {
            output.push(hex_digit(byte >> 4));
            output.push(hex_digit(byte & 0x0f));
        }
        Ok(output)
    }

    fn generate_password(&self, length: usize) -> Result<String, SetupError> {
        if length == 0 {
            return Err(SetupError::Invalid(
                "password length must be greater than zero".into(),
            ));
        }

        let password: String = rand::rng()
            .sample_iter(rand::distr::Alphanumeric)
            .take(length)
            .map(char::from)
            .collect();
        Ok(password)
    }

    fn render_env_file(
        &self,
        draft: &SetupDraftConfig,
        paths: &SetupPaths,
    ) -> Result<String, SetupError> {
        render_env_file(draft, paths)
    }

    async fn ensure_app_home_dirs(&self, paths: &SetupPaths) -> Result<(), SetupError> {
        fs::create_dir_all(&paths.app_home)
            .await
            .map_err(|e| SetupError::Io(format!("create {}: {e}", paths.app_home.display())))?;
        fs::create_dir_all(&paths.cloudflared_dir)
            .await
            .map_err(|e| {
                SetupError::Io(format!("create {}: {e}", paths.cloudflared_dir.display()))
            })?;
        Ok(())
    }

    async fn write_env_file(&self, path: &Path, contents: &str) -> Result<(), SetupError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| SetupError::Io(format!("create {}: {e}", parent.display())))?;
        }
        fs::write(path, contents)
            .await
            .map_err(|e| SetupError::Io(format!("write {}: {e}", path.display())))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .await
                .map_err(|e| {
                    SetupError::Io(format!("set permissions on {}: {e}", path.display()))
                })?;
        }

        Ok(())
    }

    async fn create_temp_log_file(&self) -> Result<PathBuf, SetupError> {
        let temp_dir = env::temp_dir();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        for attempt in 0..10 {
            let suffix = if attempt == 0 {
                format!("{timestamp}")
            } else {
                format!("{timestamp}-{attempt}")
            };
            let path = temp_dir.join(format!("brain3-{suffix}.log"));
            match fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .await
            {
                Ok(_) => return Ok(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(SetupError::Io(format!(
                        "create log file {}: {e}",
                        path.display()
                    )))
                }
            }
        }

        Err(SetupError::Io(
            "could not allocate a unique temp log file".into(),
        ))
    }

    async fn run_install_action(&self, action: InstallAction) -> Result<(), SetupError> {
        Err(SetupError::Unsupported(format!(
            "install action {action:?} is not implemented yet"
        )))
    }
}

fn operating_system() -> SetupOperatingSystem {
    match env::consts::OS {
        "macos" => SetupOperatingSystem::MacOS,
        "linux" => SetupOperatingSystem::Linux,
        other => {
            tracing::warn!(
                os = other,
                "unexpected OS for setup, defaulting to linux semantics"
            );
            SetupOperatingSystem::Linux
        }
    }
}

fn binary_on_path(name: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| {
            env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(name);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'a' + (nibble - 10)) as char,
        _ => unreachable!("nibble out of range"),
    }
}
