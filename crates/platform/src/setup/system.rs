use std::env;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use brain3_core::domain::errors::SetupError;
use brain3_core::domain::setup::{
    DependencyAvailability, DependencyStatus, InstallAction, PackageManager, SetupDraftConfig,
    SetupOperatingSystem, SetupPaths,
};
use brain3_core::ports::setup_system::SetupSystemPort;
use rand::RngExt;
use tokio::fs;
use tokio::process::Command;

use super::app_home::Brain3AppHome;
use super::env_writer::render_env_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformEnvironment {
    pub operating_system: SetupOperatingSystem,
    pub package_manager: Option<PackageManager>,
}

impl PlatformEnvironment {
    fn detect() -> Self {
        let operating_system = detect_operating_system();
        let package_manager = match operating_system {
            SetupOperatingSystem::MacOS if binary_on_path("brew") => Some(PackageManager::Homebrew),
            SetupOperatingSystem::Linux if binary_on_path("apt-get") => Some(PackageManager::Apt),
            _ => None,
        };
        Self {
            operating_system,
            package_manager,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlatformSetupSystem {
    environment: PlatformEnvironment,
    app_home_override: Option<PathBuf>,
}

impl PlatformSetupSystem {
    pub fn new() -> Self {
        Self {
            environment: PlatformEnvironment::detect(),
            app_home_override: None,
        }
    }

    pub fn with_home_override(root_dir: PathBuf) -> Self {
        Self {
            environment: PlatformEnvironment::detect(),
            app_home_override: Some(root_dir),
        }
    }

    pub fn with_environment(
        operating_system: SetupOperatingSystem,
        package_manager: Option<PackageManager>,
    ) -> Self {
        Self {
            environment: PlatformEnvironment {
                operating_system,
                package_manager,
            },
            app_home_override: None,
        }
    }
}

impl Default for PlatformSetupSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SetupSystemPort for PlatformSetupSystem {
    fn operating_system(&self) -> SetupOperatingSystem {
        self.environment.operating_system
    }

    fn resolve_paths(&self) -> Result<SetupPaths, SetupError> {
        let home = if let Some(dir) = &self.app_home_override {
            Brain3AppHome::from_root(dir.clone())
        } else {
            Brain3AppHome::resolve_from_env()?
        };
        Ok(home.as_setup_paths())
    }

    async fn collect_dependency_status(&self) -> Result<DependencyStatus, SetupError> {
        let operating_system = self.environment.operating_system;
        let homebrew_installed = match operating_system {
            SetupOperatingSystem::MacOS => Some(binary_on_path("brew")),
            SetupOperatingSystem::Linux => None,
        };
        let package_manager = self.environment.package_manager;
        let cloudflared_installed = binary_on_path("cloudflared");
        let docker_installed = binary_on_path("docker");
        let macos_container_installed = match operating_system {
            SetupOperatingSystem::MacOS => Some(binary_on_path("container")),
            SetupOperatingSystem::Linux => None,
        };
        let preferred_container_runtime = match operating_system {
            SetupOperatingSystem::MacOS => match macos_container_installed {
                Some(true) => DependencyAvailability::Installed,
                Some(false) if package_manager == Some(PackageManager::Homebrew) => {
                    DependencyAvailability::InstallAvailable(InstallAction::InstallMacOSContainer)
                }
                _ => DependencyAvailability::ManualInstallRequired,
            },
            SetupOperatingSystem::Linux => {
                if docker_installed {
                    DependencyAvailability::Installed
                } else if package_manager == Some(PackageManager::Apt) {
                    DependencyAvailability::InstallAvailable(InstallAction::InstallDocker)
                } else {
                    DependencyAvailability::ManualInstallRequired
                }
            }
        };
        let cloudflared = if cloudflared_installed {
            DependencyAvailability::Installed
        } else if matches!(
            (operating_system, package_manager),
            (SetupOperatingSystem::MacOS, Some(PackageManager::Homebrew))
                | (SetupOperatingSystem::Linux, Some(PackageManager::Apt))
        ) {
            DependencyAvailability::InstallAvailable(InstallAction::InstallCloudflared)
        } else {
            DependencyAvailability::ManualInstallRequired
        };

        Ok(DependencyStatus {
            operating_system,
            package_manager,
            cloudflared,
            preferred_container_runtime,
            docker_installed,
            macos_container_installed,
            homebrew_installed,
        })
    }

    fn generate_secret_hex(&self, num_bytes: usize) -> Result<String, SetupError> {
        use rand::Rng;

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

        const SYMBOLS: &[u8] = b"!#%^&*-_+=;:,.?~";
        const FULL: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!#%^&*-_+=;:,.?~";

        let mut rng = rand::rng();

        // Guarantee at least one symbol, fill the rest from the full charset.
        let mut bytes: Vec<u8> = std::iter::once(SYMBOLS[rng.random_range(0..SYMBOLS.len())])
            .chain((1..length).map(|_| FULL[rng.random_range(0..FULL.len())]))
            .collect();

        // Fisher-Yates shuffle so the symbol isn't always at position 0.
        for i in (1..bytes.len()).rev() {
            let j = rng.random_range(0..=i);
            bytes.swap(i, j);
        }

        String::from_utf8(bytes).map_err(|e| SetupError::Invalid(e.to_string()))
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

    async fn path_exists(&self, path: &Path) -> Result<bool, SetupError> {
        match fs::metadata(path).await {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(SetupError::Io(format!("stat {}: {error}", path.display()))),
        }
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
        match (self.environment.operating_system, self.environment.package_manager, action) {
            (SetupOperatingSystem::MacOS, Some(PackageManager::Homebrew), InstallAction::InstallCloudflared) => {
                run_command("brew", &["install", "cloudflared"]).await
            }
            (SetupOperatingSystem::MacOS, Some(PackageManager::Homebrew), InstallAction::InstallMacOSContainer) => {
                run_command("brew", &["install", "container"]).await
            }
            (SetupOperatingSystem::MacOS, _, InstallAction::InstallDocker) => Err(
                SetupError::Unsupported(
                    "guided docker install is not supported on macos; use Docker Desktop or macos-container"
                        .into(),
                ),
            ),
            (SetupOperatingSystem::MacOS, None, InstallAction::InstallCloudflared) => Err(
                SetupError::Unsupported(
                    "cloudflared install on macos requires Homebrew; install brew first and restart Brain3"
                        .into(),
                ),
            ),
            (SetupOperatingSystem::MacOS, None, InstallAction::InstallMacOSContainer) => Err(
                SetupError::Unsupported(
                    "macos container install requires Homebrew; install brew first and restart Brain3"
                        .into(),
                ),
            ),
            (SetupOperatingSystem::Linux, Some(PackageManager::Apt), InstallAction::InstallCloudflared) => {
                install_cloudflared_with_apt().await
            }
            (SetupOperatingSystem::Linux, Some(PackageManager::Apt), InstallAction::InstallDocker) => {
                install_docker_with_apt().await
            }
            (SetupOperatingSystem::Linux, _, InstallAction::InstallMacOSContainer) => Err(
                SetupError::Unsupported("macos-container is not available on linux".into()),
            ),
            (SetupOperatingSystem::Linux, None, InstallAction::InstallCloudflared) => Err(
                SetupError::Unsupported(
                    "linux cloudflared install is only guided on apt-based systems; check the README for manual install steps"
                        .into(),
                ),
            ),
            (SetupOperatingSystem::Linux, None, InstallAction::InstallDocker) => Err(
                SetupError::Unsupported(
                    "linux docker install is only guided on apt-based systems; check the README for manual install steps"
                        .into(),
                ),
            ),
            (_, _, action) => Err(SetupError::Unsupported(format!(
                "install action {action:?} is not supported on this platform"
            ))),
        }
    }
}

fn detect_operating_system() -> SetupOperatingSystem {
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

async fn install_cloudflared_with_apt() -> Result<(), SetupError> {
    run_command(
        "sudo",
        &["mkdir", "-p", "--mode=0755", "/usr/share/keyrings"],
    )
    .await?;
    run_command(
        "sudo",
        &[
            "bash",
            "-lc",
            "set -euo pipefail; curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg | tee /usr/share/keyrings/cloudflare-main.gpg >/dev/null",
        ],
    )
    .await?;
    run_command(
        "sudo",
        &[
            "bash",
            "-lc",
            "printf '%s\\n' 'deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared any main' > /etc/apt/sources.list.d/cloudflared.list",
        ],
    )
    .await?;
    run_command("sudo", &["apt-get", "update"]).await?;
    run_command("sudo", &["apt-get", "install", "-y", "cloudflared"]).await
}

async fn install_docker_with_apt() -> Result<(), SetupError> {
    run_command("sudo", &["apt-get", "update"]).await?;
    run_command(
        "sudo",
        &["apt-get", "install", "-y", "ca-certificates", "docker.io"],
    )
    .await
}

async fn run_command(program: &str, args: &[&str]) -> Result<(), SetupError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|error| SetupError::SpawnFailed(format!("{program}: {error}")))?;

    if output.status.success() {
        return Ok(());
    }

    Err(SetupError::CommandFailed {
        command: format!("{program} {}", args.join(" ")),
        code: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
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
