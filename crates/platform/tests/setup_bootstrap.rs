use std::path::PathBuf;

use brain3_core::domain::errors::SetupError;
use brain3_core::domain::model::ContainerRuntime;
use brain3_core::domain::setup::{
    AccessModeDraft, SetupDraftConfig, SetupOperatingSystem, SetupPaths, TunnelModeDraft,
};
use brain3_core::ports::setup_system::SetupSystemPort;
use brain3_platform::setup::app_home::Brain3AppHome;
use brain3_platform::setup::env_writer::render_env_file;
use brain3_platform::setup::PlatformSetupSystem;

#[test]
fn app_home_uses_brain3_home_override() {
    let app_home = Brain3AppHome::from_root(PathBuf::from("/tmp/brain3-home"));

    assert_eq!(app_home.root_dir, PathBuf::from("/tmp/brain3-home"));
    assert_eq!(app_home.env_file, PathBuf::from("/tmp/brain3-home/.env"));
    assert_eq!(
        app_home.cloudflared_dir,
        PathBuf::from("/tmp/brain3-home/cloudflared")
    );
}

#[test]
fn render_env_file_applies_setup_defaults_and_quotes_values() {
    let paths = SetupPaths::new(
        PathBuf::from("/tmp/brain3-home"),
        PathBuf::from("/tmp/brain3-home/.env"),
        PathBuf::from("/tmp/brain3-home/cloudflared"),
    );
    let draft = SetupDraftConfig {
        gateway_port: 8421,
        client_id: "custom-client".into(),
        client_secret: "secret-123".into(),
        access_token_lifetime_secs: 1234,
        refresh_token_lifetime_secs: 7776000,
        username: "admin".into(),
        password: "correct horse battery staple".into(),
        access_mode: AccessModeDraft::Both,
        tunnel_mode: TunnelModeDraft::CloudflareQuick,
        container_runtime: ContainerRuntime::MacOSContainer,
        vault_path: PathBuf::from("/Users/test/My Vault"),
        container_image_repo: "ghcr.io/tleyden/brain3-mcp-vault-tools".into(),
        container_host_port: 8420,
        container_mcp_port: 8420,
        container_network_isolated: false,
        local_mcp_enabled: true,
        local_mcp_port: 9555,
        local_mcp_bearer_token: "local-token".into(),
        pkce_required: true,
        enforce_hostname_check: true,
        direct_public_origin_hostname: None,
    };

    let rendered = render_env_file(&draft, &paths).expect("env should render");

    assert!(rendered.contains("# Set the local port for this gateway. Default: 8421."));
    assert!(rendered.contains("B3_OAUTH2_GATEWAY_CLIENT_ID=\"custom-client\""));
    assert!(rendered.contains("B3_OAUTH2_GATEWAY_CLIENT_SECRET=\"secret-123\""));
    assert!(rendered.contains("B3_OAUTH2_ACCESS_TOKEN_LIFETIME_SECS=\"1234\""));
    assert!(rendered.contains("B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS=\"7776000\""));
    assert!(rendered.contains("B3_USERNAME=\"admin\""));
    assert!(rendered.contains("B3_PASSWORD=\"correct horse battery staple\""));
    assert!(rendered.contains("B3_CF_QUICK_TUNNEL=\"true\""));
    assert!(rendered.contains("B3_LOCAL_MCP_PORT=\"9555\""));
    assert!(rendered.contains("LOCAL_GATEWAY_MCP_REVERSE_PROXY_BEARER_TOKEN=\"local-token\""));
    assert!(rendered.contains("B3_CONTAINER_RUNTIME=\"macos-container\""));
    assert!(rendered.contains("B3_VAULT_PATH=\"/Users/test/My Vault\""));
    assert!(rendered.contains("B3_CONTAINER_IMAGE_REPO=\"ghcr.io/tleyden/brain3-mcp-vault-tools\""));
    assert!(rendered.contains("B3_CONTAINER_IMAGE_TAG=\"\""));
    assert!(!rendered.contains("B3_CONTAINER_IMAGE="));
    assert!(rendered.contains("B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=\"false\""));
}

#[test]
fn render_env_file_disables_quick_tunnel_for_disabled_mode() {
    let paths = SetupPaths::new(
        PathBuf::from("/tmp/brain3-home"),
        PathBuf::from("/tmp/brain3-home/.env"),
        PathBuf::from("/tmp/brain3-home/cloudflared"),
    );
    let draft = SetupDraftConfig {
        gateway_port: 8421,
        client_id: "custom-client".into(),
        client_secret: "secret-123".into(),
        access_token_lifetime_secs: 1234,
        refresh_token_lifetime_secs: 7776000,
        username: "admin".into(),
        password: "correct horse battery staple".into(),
        access_mode: AccessModeDraft::LocalOnly,
        tunnel_mode: TunnelModeDraft::Disabled,
        container_runtime: ContainerRuntime::MacOSContainer,
        vault_path: PathBuf::from("/Users/test/My Vault"),
        container_image_repo: "ghcr.io/tleyden/brain3-mcp-vault-tools".into(),
        container_host_port: 8420,
        container_mcp_port: 8420,
        container_network_isolated: false,
        local_mcp_enabled: true,
        local_mcp_port: 8422,
        local_mcp_bearer_token: "local-token".into(),
        pkce_required: true,
        enforce_hostname_check: true,
        direct_public_origin_hostname: None,
    };

    let rendered = render_env_file(&draft, &paths).expect("env should render");

    assert!(rendered.contains("B3_CF_QUICK_TUNNEL=\"false\""));
}

#[tokio::test]
async fn run_install_action_returns_structured_error_when_platform_is_unsupported() {
    let system = PlatformSetupSystem::with_environment(SetupOperatingSystem::Linux, None);

    let error = system
        .run_install_action(brain3_core::domain::setup::InstallAction::InstallCloudflared)
        .await
        .expect_err("expected unsupported install action");

    match error {
        SetupError::Unsupported(message) => {
            assert!(message.contains("linux"));
            assert!(message.contains("cloudflared"));
        }
        other => panic!("expected unsupported setup error, got {other:?}"),
    }
}
