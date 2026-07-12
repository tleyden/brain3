use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use brain3_core::domain::model::{
    ContainerRuntime, PluginMcpContainerAuth, PluginMcpContainerConfig,
};
use serde::Deserialize;

const DEFAULT_CONTAINER_DIRECTORY: &str = "/data";
const DEFAULT_SECRET_MOUNT_PATH: &str = "/run/secrets/mcp_bearer_token";

#[derive(Debug, Deserialize)]
struct RawBrain3YamlConfig {
    #[serde(default)]
    plugin_mcp_containers: Vec<RawPluginMcpContainerConfig>,
}

#[derive(Debug, Deserialize)]
struct RawPluginMcpContainerConfig {
    name: Option<String>,
    platform: Option<String>,
    image: Option<String>,
    tag: Option<String>,
    port: Option<u16>,
    host_port: Option<u16>,
    host_directory: Option<PathBuf>,
    container_directory: Option<PathBuf>,
    auth: Option<RawPluginMcpContainerAuth>,
}

#[derive(Debug, Deserialize)]
struct RawPluginMcpContainerAuth {
    #[serde(rename = "type")]
    auth_type: Option<String>,
    secret_file: Option<PathBuf>,
    secret_mount_path: Option<PathBuf>,
}

pub fn load_plugin_mcp_containers_config(path: &Path) -> Vec<PluginMcpContainerConfig> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                path = %path.display(),
                "brain3.yaml config file not found"
            );
            return Vec::new();
        }
        Err(error) => {
            tracing::error!(
                path = %path.display(),
                error = %error,
                "failed to read brain3.yaml config file"
            );
            return Vec::new();
        }
    };

    let raw: RawBrain3YamlConfig = match serde_saphyr::from_str(&contents) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::error!(
                path = %path.display(),
                error = %error,
                "failed to parse brain3.yaml config file"
            );
            return Vec::new();
        }
    };

    validate_plugin_mcp_containers(raw.plugin_mcp_containers)
}

fn validate_plugin_mcp_containers(
    entries: Vec<RawPluginMcpContainerConfig>,
) -> Vec<PluginMcpContainerConfig> {
    let mut seen_names = HashSet::new();
    let mut configs = Vec::new();

    for entry in entries {
        let name_for_log = entry.name.as_deref().unwrap_or("<missing>").to_string();
        match validate_plugin_mcp_container(entry, &mut seen_names) {
            Ok(config) => configs.push(config),
            Err(reason) => {
                tracing::error!(
                    container = %name_for_log,
                    reason = %reason,
                    "skipping invalid Plugin MCP Container config"
                );
            }
        }
    }

    configs
}

fn validate_plugin_mcp_container(
    entry: RawPluginMcpContainerConfig,
    seen_names: &mut HashSet<String>,
) -> Result<PluginMcpContainerConfig, String> {
    let name = required_string(entry.name, "name")?;
    validate_name(&name)?;
    if !seen_names.insert(name.clone()) {
        return Err(format!("duplicate name '{name}'"));
    }

    let runtime = parse_runtime(&required_string(entry.platform, "platform")?)?;
    let image = required_string(entry.image, "image")?;
    let tag = required_string(entry.tag, "tag")?;
    let container_port = entry.port.ok_or_else(|| "missing port".to_string())?;
    let host_directory = entry
        .host_directory
        .ok_or_else(|| "missing host_directory".to_string())?;
    validate_directory(&host_directory, "host_directory")?;
    let container_directory = entry
        .container_directory
        .unwrap_or_else(|| DEFAULT_CONTAINER_DIRECTORY.into());
    let auth = parse_auth(entry.auth)?;

    Ok(PluginMcpContainerConfig {
        name,
        runtime,
        image: format!("{image}:{tag}"),
        container_port,
        host_port: entry.host_port,
        host_directory,
        container_directory,
        auth,
    })
}

fn required_string(value: Option<String>, field: &str) -> Result<String, String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing {field}"))
}

fn validate_name(name: &str) -> Result<(), String> {
    if !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Ok(());
    }

    Err("name must match [a-z0-9_]+".to_string())
}

fn parse_runtime(value: &str) -> Result<ContainerRuntime, String> {
    match value {
        "docker" => Ok(ContainerRuntime::Docker),
        "macos_container" => Ok(ContainerRuntime::MacOSContainer),
        other => Err(format!(
            "platform must be 'docker' or 'macos_container'; got '{other}'"
        )),
    }
}

fn parse_auth(auth: Option<RawPluginMcpContainerAuth>) -> Result<PluginMcpContainerAuth, String> {
    let auth = auth.ok_or_else(|| "missing auth".to_string())?;
    match required_string(auth.auth_type, "auth.type")?.as_str() {
        "none" => Ok(PluginMcpContainerAuth::None),
        "bearer_token" => {
            let secret_file = auth
                .secret_file
                .ok_or_else(|| "missing auth.secret_file".to_string())?;
            validate_readable_file(&secret_file, "auth.secret_file")?;
            Ok(PluginMcpContainerAuth::BearerToken {
                secret_file,
                secret_mount_path: auth
                    .secret_mount_path
                    .unwrap_or_else(|| DEFAULT_SECRET_MOUNT_PATH.into()),
            })
        }
        other => Err(format!(
            "auth.type must be 'none' or 'bearer_token'; got '{other}'"
        )),
    }
}

fn validate_directory(path: &Path, field: &str) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("{field} '{}' is not accessible: {error}", path.display()))?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(format!("{field} '{}' is not a directory", path.display()))
    }
}

fn validate_readable_file(path: &Path, field: &str) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("{field} '{}' is not accessible: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{field} '{}' is not a file", path.display()));
    }
    fs::File::open(path)
        .map(|_| ())
        .map_err(|error| format!("{field} '{}' is not readable: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use brain3_core::domain::model::{
        ContainerRuntime, PluginMcpContainerAuth, PluginMcpContainerConfig,
    };

    use super::load_plugin_mcp_containers_config;

    fn write_file(path: &Path, contents: &str) {
        fs::write(path, contents).expect("write test file");
    }

    fn valid_entry(name: &str, host_directory: &Path, secret_file: &Path) -> String {
        format!(
            r#"
  - name: {name}
    platform: docker
    image: ghcr.io/example/{name}
    tag: latest
    port: 8420
    host_directory: {}
    auth:
      type: bearer_token
      secret_file: {}
"#,
            host_directory.display(),
            secret_file.display()
        )
    }

    #[test]
    fn absent_file_loads_empty_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("brain3.yaml");

        let configs = load_plugin_mcp_containers_config(&path);

        assert!(configs.is_empty());
    }

    #[test]
    fn valid_multi_entry_file_loads_configs_with_defaults() {
        let temp = tempfile::tempdir().expect("tempdir");
        let first_dir = temp.path().join("first-data");
        let second_dir = temp.path().join("second-data");
        fs::create_dir_all(&first_dir).expect("first dir");
        fs::create_dir_all(&second_dir).expect("second dir");
        let first_secret = temp.path().join("first.token");
        let second_secret = temp.path().join("second.token");
        write_file(&first_secret, "first-secret");
        write_file(&second_secret, "second-secret");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"
plugin_mcp_containers:
{}
  - name: second_tool
    platform: macos_container
    image: ghcr.io/example/second
    tag: v1
    port: 9000
    host_port: 19000
    host_directory: {}
    container_directory: /workspace
    auth:
      type: none
"#,
                valid_entry("first_tool", &first_dir, &first_secret),
                second_dir.display()
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 2);
        assert_eq!(
            configs[0],
            PluginMcpContainerConfig {
                name: "first_tool".to_string(),
                runtime: ContainerRuntime::Docker,
                image: "ghcr.io/example/first_tool:latest".to_string(),
                container_port: 8420,
                host_port: None,
                host_directory: first_dir,
                container_directory: "/data".into(),
                auth: PluginMcpContainerAuth::BearerToken {
                    secret_file: first_secret,
                    secret_mount_path: "/run/secrets/mcp_bearer_token".into(),
                },
            }
        );
        assert_eq!(
            configs[1],
            PluginMcpContainerConfig {
                name: "second_tool".to_string(),
                runtime: ContainerRuntime::MacOSContainer,
                image: "ghcr.io/example/second:v1".to_string(),
                container_port: 9000,
                host_port: Some(19000),
                host_directory: second_dir,
                container_directory: "/workspace".into(),
                auth: PluginMcpContainerAuth::None,
            }
        );
    }

    #[test]
    fn missing_bearer_token_secret_file_drops_only_that_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let good_dir = temp.path().join("good-data");
        let bad_dir = temp.path().join("bad-data");
        fs::create_dir_all(&good_dir).expect("good dir");
        fs::create_dir_all(&bad_dir).expect("bad dir");
        let good_secret = temp.path().join("good.token");
        let missing_secret = temp.path().join("missing.token");
        write_file(&good_secret, "good-secret");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"
plugin_mcp_containers:
{}
{}
"#,
                valid_entry("missing_secret", &bad_dir, &missing_secret),
                valid_entry("good_tool", &good_dir, &good_secret)
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "good_tool");
    }

    #[test]
    fn duplicate_name_drops_later_duplicate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let first_dir = temp.path().join("first-data");
        let second_dir = temp.path().join("second-data");
        fs::create_dir_all(&first_dir).expect("first dir");
        fs::create_dir_all(&second_dir).expect("second dir");
        let first_secret = temp.path().join("first.token");
        let second_secret = temp.path().join("second.token");
        write_file(&first_secret, "first-secret");
        write_file(&second_secret, "second-secret");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"
plugin_mcp_containers:
{}
{}
"#,
                valid_entry("same_name", &first_dir, &first_secret),
                valid_entry("same_name", &second_dir, &second_secret)
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].host_directory, first_dir);
    }

    #[test]
    fn bad_name_charset_is_dropped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let good_dir = temp.path().join("good-data");
        let bad_dir = temp.path().join("bad-data");
        fs::create_dir_all(&good_dir).expect("good dir");
        fs::create_dir_all(&bad_dir).expect("bad dir");
        let good_secret = temp.path().join("good.token");
        let bad_secret = temp.path().join("bad.token");
        write_file(&good_secret, "good-secret");
        write_file(&bad_secret, "bad-secret");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"
plugin_mcp_containers:
{}
{}
"#,
                valid_entry("Bad-Name", &bad_dir, &bad_secret),
                valid_entry("good_name", &good_dir, &good_secret)
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "good_name");
    }

    #[test]
    fn malformed_yaml_loads_empty_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("brain3.yaml");
        write_file(&config_path, "plugin_mcp_containers:\n  - name: [");

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert!(configs.is_empty());
    }
}
