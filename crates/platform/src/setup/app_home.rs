use std::env;
use std::path::PathBuf;

use brain3_core::domain::errors::SetupError;
use brain3_core::domain::setup::SetupPaths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brain3AppHome {
    pub root_dir: PathBuf,
    pub env_file: PathBuf,
    pub cloudflared_dir: PathBuf,
}

impl Brain3AppHome {
    pub fn from_root(root_dir: PathBuf) -> Self {
        let env_file = root_dir.join(".env");
        let cloudflared_dir = root_dir.join("cloudflared");
        Self {
            root_dir,
            env_file,
            cloudflared_dir,
        }
    }

    pub fn resolve_from_env() -> Result<Self, SetupError> {
        if let Some(override_dir) = env::var_os("BRAIN3_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            return Ok(Self::from_root(override_dir));
        }

        let home_dir = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| SetupError::Invalid("HOME environment variable is not set".into()))?;

        Ok(Self::from_root(home_dir.join(".brain3")))
    }

    pub fn as_setup_paths(&self) -> SetupPaths {
        SetupPaths::new(
            self.root_dir.clone(),
            self.env_file.clone(),
            self.cloudflared_dir.clone(),
        )
    }
}
