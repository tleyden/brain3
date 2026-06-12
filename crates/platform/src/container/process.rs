use std::process::Output;

use brain3_core::domain::errors::ContainerError;
use tokio::process::Command;

async fn run_command_output(bin: &str, args: &[&str]) -> Result<Output, ContainerError> {
    tracing::debug!(cmd = bin, args = ?args, "running container command");

    Command::new(bin)
        .args(args)
        .output()
        .await
        .map_err(|e| ContainerError::SpawnFailed(format!("{bin}: {e}")))
}

pub async fn run_command(bin: &str, args: &[&str]) -> Result<String, ContainerError> {
    let output = run_command_output(bin, args).await?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        tracing::error!(cmd = bin, code, stderr, "container command failed");
        Err(ContainerError::CommandFailed { code, stderr })
    }
}

/// Run command, return true if exit 0, false if exit non-zero, err only on spawn failure.
pub async fn command_succeeds(bin: &str, args: &[&str]) -> Result<bool, ContainerError> {
    let output = run_command_output(bin, args).await?;
    Ok(output.status.success())
}
