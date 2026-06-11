use std::collections::HashMap;

use brain3_core::domain::errors::SetupError;
use brain3_core::domain::model::ContainerRuntime;
use brain3_core::domain::setup::{SetupDraftConfig, SetupPaths, TunnelModeDraft};

use super::env_template::embedded_env_template;

pub fn render_env_file(draft: &SetupDraftConfig, paths: &SetupPaths) -> Result<String, SetupError> {
    let overrides = build_overrides(draft, paths)?;
    let mut rendered = String::new();

    for line in embedded_env_template().lines() {
        if let Some((key, _)) = line.split_once('=') {
            if is_env_assignment_key(key) {
                if let Some(value) = overrides.get(key) {
                    rendered.push_str(key);
                    rendered.push('=');
                    rendered.push_str(&quote_env_value(value));
                    rendered.push('\n');
                    continue;
                }
            }
        }

        rendered.push_str(line);
        rendered.push('\n');
    }

    Ok(rendered)
}

fn build_overrides(
    draft: &SetupDraftConfig,
    paths: &SetupPaths,
) -> Result<HashMap<&'static str, String>, SetupError> {
    let mut values = HashMap::new();

    values.insert("OAUTH2_GATEWAY_PORT", draft.gateway_port.to_string());
    values.insert("OAUTH2_GATEWAY_CLIENT_ID", draft.client_id.clone());
    values.insert("OAUTH2_GATEWAY_CLIENT_SECRET", draft.client_secret.clone());
    values.insert("USERNAME", draft.username.clone());
    values.insert("PASSWORD", draft.password.clone());
    values.insert("OAUTH2_GATEWAY_ACCESS_TOKEN", draft.access_token.clone());
    values.insert(
        "BRAIN3_CONTAINER_RUNTIME",
        container_runtime_value(draft.container_runtime).to_string(),
    );
    values.insert("BRAIN3_VAULT_PATH", draft.vault_path.display().to_string());
    values.insert("BRAIN3_CONTAINER_IMAGE", draft.container_image.clone());
    values.insert(
        "BRAIN3_CONTAINER_HOST_PORT",
        draft.container_host_port.to_string(),
    );
    values.insert(
        "BRAIN3_CONTAINER_MCP_PORT",
        draft.container_mcp_port.to_string(),
    );

    match &draft.tunnel_mode {
        TunnelModeDraft::CloudflareQuick => {
            values.insert("CF_QUICK_TUNNEL", "true".into());
            values.insert("CF_TUNNEL_NAME", String::new());
            values.insert("CF_DOMAIN", String::new());
            values.insert("CF_TUNNEL_CONFIG_FILE", String::new());
            values.insert("DIRECT_PUBLIC_ORIGIN_HOSTNAME", String::new());
        }
        TunnelModeDraft::CloudflareNamed {
            tunnel_name,
            domain,
        } => {
            if tunnel_name.trim().is_empty() || domain.trim().is_empty() {
                return Err(SetupError::Invalid(
                    "named tunnel mode requires both tunnel_name and domain".into(),
                ));
            }
            values.insert("CF_QUICK_TUNNEL", "false".into());
            values.insert("CF_TUNNEL_NAME", tunnel_name.clone());
            values.insert("CF_DOMAIN", domain.clone());
            values.insert(
                "CF_TUNNEL_CONFIG_FILE",
                paths
                    .cloudflared_dir
                    .join(format!("{tunnel_name}.yml"))
                    .display()
                    .to_string(),
            );
            values.insert("DIRECT_PUBLIC_ORIGIN_HOSTNAME", String::new());
        }
        TunnelModeDraft::DirectPublicOrigin { hostname } => {
            let direct_hostname = if hostname.trim().is_empty() {
                draft
                    .direct_public_origin_hostname
                    .as_deref()
                    .unwrap_or_default()
                    .to_string()
            } else {
                hostname.clone()
            };
            if direct_hostname.trim().is_empty() {
                return Err(SetupError::Invalid(
                    "direct public origin mode requires a hostname".into(),
                ));
            }
            values.insert("CF_QUICK_TUNNEL", "false".into());
            values.insert("CF_TUNNEL_NAME", String::new());
            values.insert("CF_DOMAIN", String::new());
            values.insert("CF_TUNNEL_CONFIG_FILE", String::new());
            values.insert("DIRECT_PUBLIC_ORIGIN_HOSTNAME", direct_hostname);
        }
    }

    Ok(values)
}

fn container_runtime_value(runtime: ContainerRuntime) -> &'static str {
    match runtime {
        ContainerRuntime::Docker => "docker",
        ContainerRuntime::MacOSContainer => "macos-container",
    }
}

fn is_env_assignment_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn quote_env_value(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
