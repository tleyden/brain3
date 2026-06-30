#![cfg(feature = "e2e")]

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, ContentBlock,
        Implementation,
    },
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ServiceExt,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const OAUTH_PORT: u16 = 27630;
const LOCAL_MCP_PORT: u16 = 27640;
const CONTAINER_NAME: &str = "brain3-mcp-vault-tools";
const LOCAL_BEARER_TOKEN: &str = "e2e-test-bearer-token";

struct TempTestDir {
    root: PathBuf,
    vault: PathBuf,
    env_file: PathBuf,
    brain3_db: PathBuf,
    cloudflared_shim_dir: PathBuf,
}

impl TempTestDir {
    fn create() -> io::Result<Self> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!("brain3-e2e-{unique}"));
        let vault = root.join("vault");
        let cloudflared_shim_dir = root.join("bin");
        fs::create_dir_all(&vault)?;
        fs::create_dir_all(&cloudflared_shim_dir)?;

        let temp = Self {
            env_file: root.join(".env"),
            brain3_db: root.join("brain3.db"),
            root,
            vault,
            cloudflared_shim_dir,
        };
        temp.write_cloudflared_shim()?;
        Ok(temp)
    }

    fn write_cloudflared_shim(&self) -> io::Result<()> {
        let shim = self.cloudflared_shim_dir.join("cloudflared");
        fs::write(&shim, "#!/bin/sh\nexit 0\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }

    fn write_env_file(&self) -> io::Result<()> {
        fs::write(
            &self.env_file,
            format!(
                "B3_OAUTH2_GATEWAY_PORT={OAUTH_PORT}\n\
                 B3_OAUTH2_GATEWAY_CLIENT_SECRET=e2e-test-client-secret\n\
                 B3_USERNAME=e2e-test-user\n\
                 B3_PASSWORD=e2e-test-password\n\
                 B3_TOKEN_DB_PATH={}\n\
                 B3_CF_QUICK_TUNNEL=false\n\
                 B3_CONTAINER_RUNTIME=docker\n\
                 B3_VAULT_PATH={}\n\
                 B3_CONTAINER_IMAGE_REPO=brain3-mcp-vault-tools\n\
                 B3_CONTAINER_IMAGE_TAG=e2e-local\n\
                 B3_UPSTREAM_SHARED_SECRET=e2e-test-upstream-secret\n\
                 B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false\n\
                 B3_LOCAL_MCP_PORT={LOCAL_MCP_PORT}\n\
                 LOCAL_GATEWAY_MCP_BEARER_TOKEN={LOCAL_BEARER_TOKEN}\n\
                 B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK=false\n",
                self.brain3_db.display(),
                self.vault.display(),
            ),
        )
    }

    fn path_with_shim(&self) -> String {
        let mut paths = vec![self.cloudflared_shim_dir.clone()];
        if let Some(existing) = env::var_os("PATH") {
            paths.extend(env::split_paths(&existing));
        }
        env::join_paths(paths)
            .expect("test PATH should be joinable")
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Brain3Process {
    child: Child,
}

impl Brain3Process {
    async fn spawn(temp: &TempTestDir) -> Result<Self, Box<dyn std::error::Error>> {
        let binary = env!("CARGO_BIN_EXE_brain3");
        let child = Command::new(binary)
            .arg("--cli")
            .arg("--env-file")
            .arg(&temp.env_file)
            .arg("--brain3-home")
            .arg(&temp.root)
            .arg("--log-level")
            .arg("debug")
            .env("PATH", temp.path_with_shim())
            .env("B3_HOME", &temp.root)
            .env("B3_OAUTH2_GATEWAY_PORT", OAUTH_PORT.to_string())
            .env("B3_OAUTH2_GATEWAY_CLIENT_SECRET", "e2e-test-client-secret")
            .env("B3_USERNAME", "e2e-test-user")
            .env("B3_PASSWORD", "e2e-test-password")
            .env("B3_TOKEN_DB_PATH", &temp.brain3_db)
            .env("B3_CF_QUICK_TUNNEL", "false")
            .env("B3_CONTAINER_RUNTIME", "docker")
            .env("B3_VAULT_PATH", &temp.vault)
            .env("B3_CONTAINER_IMAGE_REPO", "brain3-mcp-vault-tools")
            .env("B3_CONTAINER_IMAGE_TAG", "e2e-local")
            .env("B3_UPSTREAM_SHARED_SECRET", "e2e-test-upstream-secret")
            .env("B3_CONTAINER_INTERNAL_NETWORK_ISOLATION", "false")
            .env("B3_LOCAL_MCP_PORT", LOCAL_MCP_PORT.to_string())
            .env("LOCAL_GATEWAY_MCP_BEARER_TOKEN", LOCAL_BEARER_TOKEN)
            .env("B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK", "false")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        let process = Self { child };
        process.wait_for_health().await?;
        Ok(process)
    }

    async fn wait_for_health(&self) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last_error = String::from("health endpoint was not probed");

        while Instant::now() < deadline {
            match probe_health().await {
                Ok(()) => return Ok(()),
                Err(error) => last_error = error.to_string(),
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        Err(format!("gateway did not become healthy within 30s: {last_error}").into())
    }
}

async fn probe_health() -> io::Result<()> {
    let mut stream = TcpStream::connect(("127.0.0.1", OAUTH_PORT)).await?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await?;

    let mut response = vec![0; 128];
    let read = stream.read(&mut response).await?;
    let response = String::from_utf8_lossy(&response[..read]);
    if response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "health endpoint returned non-200 response: {response}"
        )))
    }
}

impl Drop for Brain3Process {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }

        let pid = self.child.id().to_string();
        let _ = Command::new("kill").arg("-INT").arg(&pid).status();

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn e2e_smoke_local_docker() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempTestDir::create()?;
    temp.write_env_file()?;

    let gateway = Brain3Process::spawn(&temp).await?;
    let client = connect_local_mcp().await?;

    let tools = client.list_tools(Default::default()).await?;
    let tool_names = tools
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<HashSet<_>>();
    for expected in [
        "vault_create_overwrite_file",
        "vault_read",
        "vault_apply_unified_diff",
        "vault_search",
        "vault_delete",
    ] {
        assert!(tool_names.contains(expected), "missing MCP tool {expected}");
    }

    let create = call_tool_json(
        &client,
        "vault_create_overwrite_file",
        json!({
            "path": "e2e-test/note.md",
            "content": "# E2E Test\nHello world.",
        }),
    )
    .await?;
    assert_eq!(create["path"], "e2e-test/note.md");
    assert_eq!(create["created"], true);

    let read = call_tool_json(&client, "vault_read", json!({"path": "e2e-test/note.md"})).await?;
    assert!(
        read["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Hello world"),
        "read content did not contain created text: {read}"
    );

    let update = call_tool_json(
        &client,
        "vault_apply_unified_diff",
        json!({
            "path": "e2e-test/note.md",
            "diff": "@@ -2,1 +2,1 @@\n-Hello world.\n+Hello updated world.",
        }),
    )
    .await?;
    assert_eq!(update["applied"], true);

    let reread = call_tool_json(&client, "vault_read", json!({"path": "e2e-test/note.md"})).await?;
    assert!(
        reread["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Hello updated world"),
        "read content did not contain updated text: {reread}"
    );

    let search = call_tool_json(
        &client,
        "vault_search",
        json!({"query": "updated world", "max_results": 5}),
    )
    .await?;
    let search_text = serde_json::to_string(&search)?;
    assert!(
        search_text.contains("e2e-test/note.md"),
        "search result did not reference test note: {search_text}"
    );

    let delete = call_tool_json(
        &client,
        "vault_delete",
        json!({"path": "e2e-test/note.md", "confirm": true}),
    )
    .await?;
    assert_eq!(delete["deleted"], true);

    let deleted_read =
        call_tool_json(&client, "vault_read", json!({"path": "e2e-test/note.md"})).await?;
    assert!(
        deleted_read.get("error").is_some(),
        "post-delete read should return an error payload: {deleted_read}"
    );

    client.cancel().await?;
    drop(gateway);
    assert_no_container_residue().await?;
    Ok(())
}

async fn connect_local_mcp(
) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>, Box<dyn std::error::Error>>
{
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!(
            "http://127.0.0.1:{LOCAL_MCP_PORT}/mcp"
        ))
        .auth_header(LOCAL_BEARER_TOKEN),
    );
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("brain3-e2e-smoke", "0.0.0"),
    );

    Ok(client_info.serve(transport).await?)
}

async fn call_tool_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>,
    name: &'static str,
    arguments: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let arguments = arguments
        .as_object()
        .cloned()
        .ok_or("tool arguments must be a JSON object")?;
    let result = client
        .call_tool(CallToolRequestParams::new(name).with_arguments(arguments))
        .await?;
    assert!(
        result.is_error != Some(true),
        "tool {name} returned MCP error result: {result:?}"
    );
    Ok(tool_result_json(&result)?)
}

fn tool_result_json(result: &CallToolResult) -> Result<Value, Box<dyn std::error::Error>> {
    let text = result
        .content
        .iter()
        .find_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .ok_or("tool result did not include text content")?;
    Ok(serde_json::from_str(text)?)
}

async fn assert_no_container_residue() -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last_output = String::new();

    while Instant::now() < deadline {
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                &format!("name={CONTAINER_NAME}"),
                "--format",
                "{{.Names}}",
            ])
            .output()?;
        last_output = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if output.status.success() && last_output.is_empty() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    Err(format!("managed MCP container residue remained after shutdown: {last_output}").into())
}
