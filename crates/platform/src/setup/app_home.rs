use std::env;
use std::path::PathBuf;

use brain3_core::domain::errors::SetupError;
use brain3_core::domain::setup::SetupPaths;

use crate::util::user_home_dir;

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
        if let Some(override_dir) = env::var_os("B3_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            return Ok(Self::from_root(override_dir));
        }

        let home_dir = user_home_dir()
            .ok_or_else(|| SetupError::Invalid("neither HOME nor USERPROFILE is set".into()))?;

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

#[cfg(test)]
mod tests {
    use std::sync::{LazyLock, Mutex};

    use super::*;

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
    const HOME_KEYS: &[&str] = &["HOME", "USERPROFILE", "B3_HOME"];

    fn with_clean_home_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
        let saved: Vec<(&str, Option<String>)> = HOME_KEYS
            .iter()
            .map(|key| (*key, env::var(key).ok()))
            .collect();

        for key in HOME_KEYS {
            env::remove_var(key);
        }

        let result = f();

        for key in HOME_KEYS {
            env::remove_var(key);
        }
        for (key, value) in saved {
            if let Some(value) = value {
                env::set_var(key, value);
            }
        }

        result
    }

    #[test]
    fn resolve_from_env_uses_cross_platform_home_precedence() {
        with_clean_home_env(|| {
            env::set_var("USERPROFILE", "/tmp/brain3-userprofile");
            assert_eq!(
                Brain3AppHome::resolve_from_env()
                    .expect("USERPROFILE should provide default app home")
                    .root_dir,
                PathBuf::from("/tmp/brain3-userprofile/.brain3")
            );

            env::set_var("HOME", "/tmp/brain3-home");
            assert_eq!(
                Brain3AppHome::resolve_from_env()
                    .expect("HOME should provide default app home")
                    .root_dir,
                PathBuf::from("/tmp/brain3-home/.brain3")
            );

            env::set_var("B3_HOME", "/tmp/brain3-override");
            assert_eq!(
                Brain3AppHome::resolve_from_env()
                    .expect("B3_HOME should override default app home")
                    .root_dir,
                PathBuf::from("/tmp/brain3-override")
            );
        });
    }
}
