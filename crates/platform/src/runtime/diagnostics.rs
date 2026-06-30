use std::io::{self, Write};
use std::sync::Arc;

use brain3_core::domain::model::GatewayConfig;
use brain3_core::ports::container::ContainerId;

use crate::container::startup::container_port_for_runtime;

const DIAGNOSTIC_LOG_LINES: usize = 10_000;

pub(crate) fn container_diagnostics_end_sentinel(container_name: &str) -> String {
    format!("=== end brain3 container diagnostics: {container_name} ===\n")
}

fn bool_status(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

pub(crate) fn format_container_diagnostics_dump(
    container_name: &str,
    exists: Option<bool>,
    running: Option<bool>,
    logs: Result<String, String>,
) -> String {
    let mut dump = format!(
        "=== brain3 container diagnostics: {container_name} ===\n\
         exists: {}\n\
         running: {}\n",
        bool_status(exists),
        bool_status(running)
    );

    match logs {
        Ok(logs) if logs.is_empty() => {
            dump.push_str(&format!(
                "Container logs (tail {DIAGNOSTIC_LOG_LINES}):\n<empty>\n"
            ));
        }
        Ok(logs) => {
            dump.push_str(&format!("Container logs (tail {DIAGNOSTIC_LOG_LINES}):\n"));
            dump.push_str(&logs);
            if !logs.ends_with('\n') {
                dump.push('\n');
            }
        }
        Err(error) => {
            dump.push_str("--- logs error ---\n");
            dump.push_str(&error);
            if !error.ends_with('\n') {
                dump.push('\n');
            }
        }
    }

    dump.push_str(&container_diagnostics_end_sentinel(container_name));
    dump
}

pub async fn dump_container_diagnostics(config: &GatewayConfig) {
    let Some(startup) = config.container.as_ref() else {
        tracing::info!("SIGUSR1 diagnostics requested, but no managed MCP container is configured");
        return;
    };

    let container_name = startup.container_name.as_str();
    let id = ContainerId(startup.container_name.clone());
    let port = container_port_for_runtime(startup.runtime);

    let exists = match port.exists(&id).await {
        Ok(exists) => Some(exists),
        Err(error) => {
            tracing::warn!(
                container = %container_name,
                error = %error,
                "failed to inspect MCP container existence during diagnostics dump"
            );
            None
        }
    };
    let running = match port.is_running(&id).await {
        Ok(running) => Some(running),
        Err(error) => {
            tracing::warn!(
                container = %container_name,
                error = %error,
                "failed to inspect MCP container running state during diagnostics dump"
            );
            None
        }
    };
    let logs = port
        .logs_tail(&id, DIAGNOSTIC_LOG_LINES)
        .await
        .map_err(|error| error.to_string());

    let dump = format_container_diagnostics_dump(container_name, exists, running, logs);
    print!("{dump}");
    if let Err(error) = io::stdout().flush() {
        tracing::warn!(
            container = %container_name,
            error = %error,
            "failed to flush container diagnostics dump to stdout"
        );
    }
    tracing::info!(container = %container_name, "dumped container diagnostics on SIGUSR1");
}

#[cfg(unix)]
pub fn spawn_diagnostics_signal_listener(
    config: Arc<GatewayConfig>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut sig = match tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::user_defined1(),
        ) {
            Ok(sig) => sig,
            Err(error) => {
                tracing::warn!(error = %error, "failed to install SIGUSR1 diagnostics listener");
                return;
            }
        };

        while sig.recv().await.is_some() {
            dump_container_diagnostics(&config).await;
        }
    })
}

#[cfg(not(unix))]
pub fn spawn_diagnostics_signal_listener(
    _config: Arc<GatewayConfig>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_dump_includes_state_logs_and_stable_sentinel() {
        let dump = format_container_diagnostics_dump(
            "brain3-mcp-vault-tools",
            Some(true),
            Some(true),
            Ok("line one\nline two\n".to_string()),
        );

        assert!(dump.starts_with("=== brain3 container diagnostics: brain3-mcp-vault-tools ===\n"));
        assert!(dump.contains("exists: true\n"));
        assert!(dump.contains("running: true\n"));
        assert!(dump.contains("Container logs (tail 10000):\nline one\nline two\n"));
        assert!(
            dump.ends_with("=== end brain3 container diagnostics: brain3-mcp-vault-tools ===\n")
        );
    }

    #[test]
    fn diagnostics_dump_reports_log_errors_before_sentinel() {
        let dump = format_container_diagnostics_dump(
            "brain3-mcp-vault-tools",
            None,
            None,
            Err("docker logs failed".to_string()),
        );

        assert!(dump.contains("exists: unknown\n"));
        assert!(dump.contains("running: unknown\n"));
        assert!(dump.contains("--- logs error ---\ndocker logs failed\n"));
        assert!(dump.ends_with(&container_diagnostics_end_sentinel(
            "brain3-mcp-vault-tools"
        )));
    }
}
