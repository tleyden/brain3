use std::path::Path;

use anyhow::{Context, Result};
use rand::Rng;

pub fn read_or_create(path: &Path) -> Result<String> {
    let path_str = path
        .to_str()
        .context("upstream secret file path is not valid UTF-8")?;
    if path_str.trim().is_empty() {
        anyhow::bail!("MCP upstream shared secret file path is empty");
    }

    if path.exists() {
        let secret = std::fs::read_to_string(path)
            .with_context(|| {
                format!(
                    "Unable to read MCP upstream shared secret file: {}",
                    path.display()
                )
            })?
            .trim()
            .to_string();
        if !secret.is_empty() {
            return Ok(secret);
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Unable to create directory for upstream secret: {}",
                parent.display()
            )
        })?;
    }

    let secret: String = rand::rng()
        .sample_iter(rand::distr::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();

    std::fs::write(path, &secret).with_context(|| {
        format!("Unable to write upstream secret file: {}", path.display())
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Unable to set permissions on: {}", path.display()))?;
    }

    tracing::info!("Generated upstream shared secret at {}", path.display());
    Ok(secret)
}
