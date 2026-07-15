use std::collections::{BTreeMap, HashSet};
use std::env;
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
    network: Option<String>,
    network_isolation: Option<bool>,
    env: Option<BTreeMap<String, String>>,
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
    let network_isolation = entry.network_isolation.unwrap_or(true);
    validate_plugin_network_isolation_support(runtime, network_isolation)?;
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
    let network_name = required_string(entry.network, "network")?;
    validate_network_name(&network_name)?;
    let env = parse_env(entry.env)?;
    let auth = parse_auth(entry.auth)?;

    Ok(PluginMcpContainerConfig {
        name,
        runtime,
        image: format!("{image}:{tag}"),
        container_port,
        host_port: entry.host_port,
        host_directory,
        container_directory,
        network_name,
        network_isolation,
        env,
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

fn validate_network_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));

    if first_ok && rest_ok {
        Ok(())
    } else {
        Err(
            "network must start with a letter/digit and contain only letters, digits, '_', '.', '-'"
                .to_string(),
        )
    }
}

fn parse_env(env: Option<BTreeMap<String, String>>) -> Result<Vec<(String, String)>, String> {
    let Some(env) = env else {
        return Ok(Vec::new());
    };

    for key in env.keys() {
        validate_env_var_name(key)?;
    }

    Ok(env.into_iter().collect())
}

fn validate_env_var_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');

    if first_ok && rest_ok {
        Ok(())
    } else {
        Err(format!(
            "env variable name '{name}' must match [A-Za-z_][A-Za-z0-9_]*"
        ))
    }
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

fn validate_plugin_network_isolation_support(
    runtime: ContainerRuntime,
    network_isolation: bool,
) -> Result<(), String> {
    if network_isolation
        && matches!(runtime, ContainerRuntime::Docker)
        && env::consts::OS == "macos"
    {
        return Err(
            "network_isolation: true is not supported with platform: docker on macOS; \
             set network_isolation: false or platform: macos_container for this plugin"
                .to_string(),
        );
    }

    Ok(())
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

    fn valid_entry(name: &str, network: &str, host_directory: &Path, secret_file: &Path) -> String {
        format!(
            r#"
  - name: {name}
    platform: macos_container
    image: ghcr.io/example/{name}
    tag: latest
    port: 8420
    host_directory: {}
    network: {network}
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
    network: second-tool-net
    auth:
      type: none
"#,
                valid_entry("first_tool", "first-tool-net", &first_dir, &first_secret),
                second_dir.display()
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 2);
        assert_eq!(
            configs[0],
            PluginMcpContainerConfig {
                name: "first_tool".to_string(),
                runtime: ContainerRuntime::MacOSContainer,
                image: "ghcr.io/example/first_tool:latest".to_string(),
                container_port: 8420,
                host_port: None,
                host_directory: first_dir,
                container_directory: "/data".into(),
                network_name: "first-tool-net".into(),
                network_isolation: true,
                env: Vec::new(),
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
                network_name: "second-tool-net".into(),
                network_isolation: true,
                env: Vec::new(),
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
                valid_entry(
                    "missing_secret",
                    "missing-secret-net",
                    &bad_dir,
                    &missing_secret
                ),
                valid_entry("good_tool", "good-tool-net", &good_dir, &good_secret)
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
                valid_entry("same_name", "first-net", &first_dir, &first_secret),
                valid_entry("same_name", "second-net", &second_dir, &second_secret)
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
                valid_entry("Bad-Name", "bad-name-net", &bad_dir, &bad_secret),
                valid_entry("good_name", "good-name-net", &good_dir, &good_secret)
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "good_name");
    }

    #[test]
    fn env_map_loads_in_deterministic_key_order() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host_dir = temp.path().join("data");
        fs::create_dir_all(&host_dir).expect("host dir");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"plugin_mcp_containers:
  - name: env_plugin
    platform: macos_container
    image: ghcr.io/example/env-plugin
    tag: latest
    port: 8420
    host_directory: {}
    network: env-plugin-net
    env:
      FOO: bar
      BAZ: qux
    auth:
      type: none
"#,
                host_dir.display()
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert_eq!(
            configs[0].env,
            vec![
                ("BAZ".to_string(), "qux".to_string()),
                ("FOO".to_string(), "bar".to_string()),
            ]
        );
    }

    #[test]
    fn invalid_env_key_drops_only_that_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let good_dir = temp.path().join("good-data");
        let bad_dir = temp.path().join("bad-data");
        fs::create_dir_all(&good_dir).expect("good dir");
        fs::create_dir_all(&bad_dir).expect("bad dir");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"plugin_mcp_containers:
  - name: bad_env
    platform: macos_container
    image: ghcr.io/example/bad-env
    tag: latest
    port: 8420
    host_directory: {}
    network: bad-env-net
    env:
      1BAD: x
    auth:
      type: none
  - name: good_env
    platform: macos_container
    image: ghcr.io/example/good-env
    tag: latest
    port: 8420
    host_directory: {}
    network: good-env-net
    auth:
      type: none
"#,
                bad_dir.display(),
                good_dir.display()
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "good_env");
    }

    #[test]
    fn missing_env_loads_empty_env() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host_dir = temp.path().join("data");
        fs::create_dir_all(&host_dir).expect("host dir");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"plugin_mcp_containers:
  - name: no_env
    platform: macos_container
    image: ghcr.io/example/no-env
    tag: latest
    port: 8420
    host_directory: {}
    network: no-env-net
    auth:
      type: none
"#,
                host_dir.display()
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].env, Vec::<(String, String)>::new());
    }

    #[test]
    fn missing_network_is_dropped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host_dir = temp.path().join("data");
        fs::create_dir_all(&host_dir).expect("host dir");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"
plugin_mcp_containers:
  - name: missing_network
    platform: docker
    image: ghcr.io/example/missing-network
    tag: latest
    port: 8420
    host_directory: {}
    auth:
      type: none
"#,
                host_dir.display()
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert!(configs.is_empty());
    }

    #[test]
    fn invalid_network_name_is_dropped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host_dir = temp.path().join("data");
        fs::create_dir_all(&host_dir).expect("host dir");
        let secret_file = temp.path().join("token");
        write_file(&secret_file, "secret");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                "plugin_mcp_containers:\n{}",
                valid_entry("bad_network", "-leading-hyphen", &host_dir, &secret_file)
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert!(configs.is_empty());
    }

    #[test]
    fn docker_without_network_isolation_loads_on_all_platforms() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host_dir = temp.path().join("data");
        fs::create_dir_all(&host_dir).expect("host dir");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"plugin_mcp_containers:
  - name: egress_plugin
    platform: docker
    image: ghcr.io/example/egress-plugin
    tag: latest
    port: 8420
    host_directory: {}
    network: egress-plugin-net
    network_isolation: false
    auth:
      type: none
"#,
                host_dir.display()
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].runtime, ContainerRuntime::Docker);
        assert!(!configs[0].network_isolation);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn docker_with_network_isolation_drops_only_that_entry_on_macos() {
        let temp = tempfile::tempdir().expect("tempdir");
        let docker_dir = temp.path().join("docker-data");
        let native_dir = temp.path().join("native-data");
        fs::create_dir_all(&docker_dir).expect("docker dir");
        fs::create_dir_all(&native_dir).expect("native dir");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"plugin_mcp_containers:
  - name: isolated_docker
    platform: docker
    image: ghcr.io/example/isolated-docker
    tag: latest
    port: 8420
    host_directory: {}
    network: isolated-docker-net
    auth:
      type: none
  - name: isolated_native
    platform: macos_container
    image: ghcr.io/example/isolated-native
    tag: latest
    port: 8420
    host_directory: {}
    network: isolated-native-net
    auth:
      type: none
"#,
                docker_dir.display(),
                native_dir.display()
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "isolated_native");
        assert!(configs[0].network_isolation);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn docker_with_network_isolation_loads_on_linux() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host_dir = temp.path().join("data");
        fs::create_dir_all(&host_dir).expect("host dir");
        let config_path = temp.path().join("brain3.yaml");
        write_file(
            &config_path,
            &format!(
                r#"plugin_mcp_containers:
  - name: isolated_docker
    platform: docker
    image: ghcr.io/example/isolated-docker
    tag: latest
    port: 8420
    host_directory: {}
    network: isolated-docker-net
    auth:
      type: none
"#,
                host_dir.display()
            ),
        );

        let configs = load_plugin_mcp_containers_config(&config_path);

        assert_eq!(configs.len(), 1);
        assert!(configs[0].network_isolation);
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
