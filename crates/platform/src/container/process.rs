use std::process::Output;

use brain3_core::domain::errors::ContainerError;
use tokio::process::Command;

pub(super) fn format_launch_command(bin: &str, args: &[String]) -> String {
    let mut redact_next_env = false;
    std::iter::once(bin.to_string())
        .chain(args.iter().map(|arg| {
            if redact_next_env {
                redact_next_env = false;
                let key = arg.split_once('=').map_or(arg.as_str(), |(key, _)| key);
                return format!("{key}=<redacted>");
            }
            if arg == "--env" {
                redact_next_env = true;
            }
            arg.clone()
        }))
        .map(|part| shell_quote(&part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-._/:=@%+,".contains(ch))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

async fn run_command_output(bin: &str, args: &[&str]) -> Result<Output, ContainerError> {
    let owned_args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    tracing::debug!(
        command = %format_launch_command(bin, &owned_args),
        "running container command"
    );

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
