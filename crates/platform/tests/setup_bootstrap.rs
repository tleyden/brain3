use std::path::PathBuf;

use brain3_core::domain::model::ContainerRuntime;
use brain3_core::domain::setup::{SetupDraftConfig, SetupPaths, TunnelModeDraft};
use brain3_platform::setup::app_home::Brain3AppHome;
use brain3_platform::setup::env_writer::render_env_file;

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
        access_token: "access-456".into(),
        username: "admin".into(),
        password: "correct horse battery staple".into(),
        tunnel_mode: TunnelModeDraft::CloudflareQuick,
        container_runtime: ContainerRuntime::MacOSContainer,
        vault_path: PathBuf::from("/Users/test/My Vault"),
        container_image: "ghcr.io/tleyden/brain3-mcp-vault-tools:latest".into(),
        container_host_port: 8420,
        container_mcp_port: 8420,
        direct_public_origin_hostname: None,
    };

    let rendered = render_env_file(&draft, &paths).expect("env should render");

    assert!(rendered.contains("# Set the local port for this gateway. Default: 8421."));
    assert!(rendered.contains("OAUTH2_GATEWAY_CLIENT_ID=\"custom-client\""));
    assert!(rendered.contains("OAUTH2_GATEWAY_CLIENT_SECRET=\"secret-123\""));
    assert!(rendered.contains("OAUTH2_GATEWAY_ACCESS_TOKEN=\"access-456\""));
    assert!(rendered.contains("USERNAME=\"admin\""));
    assert!(rendered.contains("PASSWORD=\"correct horse battery staple\""));
    assert!(rendered.contains("CF_QUICK_TUNNEL=\"true\""));
    assert!(rendered.contains("BRAIN3_CONTAINER_RUNTIME=\"macos-container\""));
    assert!(rendered.contains("BRAIN3_VAULT_PATH=\"/Users/test/My Vault\""));
    assert!(rendered
        .contains("BRAIN3_CONTAINER_IMAGE=\"ghcr.io/tleyden/brain3-mcp-vault-tools:latest\""));
}
