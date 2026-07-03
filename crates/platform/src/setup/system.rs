use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use brain3_core::domain::errors::SetupError;
use brain3_core::domain::setup::{
    DependencyAvailability, DependencyStatus, InstallAction, PackageManager, SetupDraftConfig,
    SetupOperatingSystem, SetupPaths,
};
use brain3_core::ports::setup_system::SetupSystemPort;
use rand::RngExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::app_home::Brain3AppHome;
use super::env_writer::render_env_file;

const WHISPER_MODEL_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

struct WhisperModelSpec {
    model: &'static str,
    filename: &'static str,
    url: &'static str,
    sha256: &'static str,
    size_bytes: u64,
}

const WHISPER_MODEL_SPECS: &[WhisperModelSpec] = &[
    WhisperModelSpec {
        model: "tiny.en",
        filename: "ggml-tiny.en.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
        sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
        size_bytes: 77_704_715,
    },
    WhisperModelSpec {
        model: "base.en",
        filename: "ggml-base.en.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        size_bytes: 147_964_211,
    },
];

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

    async fn ensure_whisper_model(
        &self,
        paths: &SetupPaths,
        model: &str,
    ) -> Result<PathBuf, SetupError> {
        let spec = whisper_model_spec(model)?;
        let model_dir = paths.app_home.join("whisper-models");
        let model_path = model_dir.join(spec.filename);

        fs::create_dir_all(&model_dir)
            .await
            .map_err(|e| SetupError::Io(format!("create {}: {e}", model_dir.display())))?;

        if model_path.is_file() {
            verify_whisper_model_file(&model_path, spec).map_err(|error| {
                SetupError::Invalid(format!(
                    "existing Whisper model verification failed at {}: {error}",
                    model_path.display()
                ))
            })?;
            tracing::info!(
                model = spec.model,
                path = %model_path.display(),
                "Whisper model already downloaded and verified"
            );
            return Ok(model_path);
        }

        download_whisper_model(&model_path, spec).await?;
        Ok(model_path)
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

    async fn resolve_log_file(&self, paths: &SetupPaths) -> Result<PathBuf, SetupError> {
        if let Err(error) = self.ensure_app_home_dirs(paths).await {
            tracing::warn!(
                path = %paths.app_home.display(),
                error = %error,
                "could not create app home dirs for log file, falling back to temp dir"
            );
            return Ok(env::temp_dir().join("brain3.log"));
        }

        Ok(paths.app_home.join("brain3.log"))
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

fn whisper_model_spec(model: &str) -> Result<&'static WhisperModelSpec, SetupError> {
    WHISPER_MODEL_SPECS
        .iter()
        .find(|spec| spec.model == model)
        .ok_or_else(|| {
            SetupError::Invalid(format!(
                "unsupported Whisper model '{model}'; choose tiny.en or base.en"
            ))
        })
}

async fn download_whisper_model(
    model_path: &Path,
    spec: &WhisperModelSpec,
) -> Result<(), SetupError> {
    let partial_path = model_path.with_extension("bin.download");
    let _ = fs::remove_file(&partial_path).await;

    tracing::info!(
        model = spec.model,
        url = spec.url,
        path = %model_path.display(),
        expected_bytes = spec.size_bytes,
        "downloading Whisper model"
    );

    let client = reqwest::Client::builder()
        .timeout(WHISPER_MODEL_DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| SetupError::Io(format!("create Whisper model HTTP client: {e}")))?;
    let mut response = client
        .get(spec.url)
        .send()
        .await
        .map_err(|e| SetupError::Io(format!("download Whisper model {}: {e}", spec.model)))?;

    if !response.status().is_success() {
        return Err(SetupError::Io(format!(
            "download Whisper model {}: HTTP {}",
            spec.model,
            response.status()
        )));
    }

    if let Some(content_length) = response.content_length() {
        if content_length != spec.size_bytes {
            return Err(SetupError::Invalid(format!(
                "Whisper model {} size changed before download: expected {}, got {}",
                spec.model, spec.size_bytes, content_length
            )));
        }
    }

    let mut file = fs::File::create(&partial_path)
        .await
        .map_err(|e| SetupError::Io(format!("create {}: {e}", partial_path.display())))?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0_u64;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| SetupError::Io(format!("read Whisper model {}: {e}", spec.model)))?
    {
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| SetupError::Invalid("Whisper model byte count overflowed".into()))?;
        if downloaded > spec.size_bytes {
            return Err(SetupError::Invalid(format!(
                "Whisper model {} exceeded expected size: {} > {}",
                spec.model, downloaded, spec.size_bytes
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| SetupError::Io(format!("write {}: {e}", partial_path.display())))?;
    }
    file.flush()
        .await
        .map_err(|e| SetupError::Io(format!("flush {}: {e}", partial_path.display())))?;

    verify_whisper_model_hash(downloaded, &hasher.finalize(), spec)?;
    fs::rename(&partial_path, model_path).await.map_err(|e| {
        SetupError::Io(format!(
            "move {} to {}: {e}",
            partial_path.display(),
            model_path.display()
        ))
    })?;

    tracing::info!(
        model = spec.model,
        path = %model_path.display(),
        bytes = downloaded,
        "Whisper model downloaded and verified"
    );
    Ok(())
}

fn verify_whisper_model_file(path: &Path, spec: &WhisperModelSpec) -> Result<(), String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes = 0_u64;

    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| "Whisper model byte count overflowed".to_string())?;
        hasher.update(&buffer[..read]);
    }

    verify_whisper_model_hash(bytes, &hasher.finalize(), spec).map_err(|error| error.to_string())
}

fn verify_whisper_model_hash(
    bytes: u64,
    actual_hash: &[u8],
    spec: &WhisperModelSpec,
) -> Result<(), SetupError> {
    if bytes != spec.size_bytes {
        return Err(SetupError::Invalid(format!(
            "Whisper model {} size mismatch: expected {}, got {}",
            spec.model, spec.size_bytes, bytes
        )));
    }

    let actual_hash = actual_hash
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual_hash != spec.sha256 {
        return Err(SetupError::Invalid(format!(
            "Whisper model {} SHA-256 mismatch: expected {}, got {}",
            spec.model, spec.sha256, actual_hash
        )));
    }

    Ok(())
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
