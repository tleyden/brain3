#![cfg(feature = "e2e")]

use std::collections::BTreeSet;
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
    assert_container_running_and_vault_visible().await?;
    let client = connect_local_mcp().await?;

    let tools = client.list_tools(Default::default()).await?;
    let tool_names = tools
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<BTreeSet<_>>();
    let expected_tool_names = BTreeSet::from([
        "vault_apply_unified_diff",
        "vault_batch_frontmatter_update",
        "vault_batch_read",
        "vault_create_overwrite_file",
        "vault_delete",
        "vault_list",
        "vault_move",
        "vault_read",
        "vault_search",
        "vault_search_frontmatter",
    ]);
    assert_eq!(tool_names, expected_tool_names);

    for (path, content) in [
        (
            "projects/alpha.md",
            "---\nstatus: draft\ntags:\n  - work\n---\n# Alpha\nAlpha kickoff details.\n",
        ),
        (
            "projects/beta.md",
            "---\nstatus: draft\n---\n# Beta\nBeta planning details.\n",
        ),
        (
            "daily/2026-06-30.md",
            "# 2026-06-30\nDaily note for project planning.\n",
        ),
    ] {
        let create = call_tool_json(
            &client,
            "vault_create_overwrite_file",
            json!({
                "path": path,
                "content": content,
            }),
        )
        .await?;
        assert_eq!(create["path"], path);
        assert_eq!(create["created"], true);
    }

    let project_list = call_tool_json(
        &client,
        "vault_list",
        json!({"path": "projects", "depth": 1}),
    )
    .await?;
    assert!(
        project_list["total"].as_u64().unwrap_or_default() >= 2,
        "projects listing should include at least alpha and beta: {project_list}"
    );
    let project_list_paths = json_result_paths(&project_list, "items")?;
    assert!(
        project_list_paths.contains("projects/alpha.md")
            && project_list_paths.contains("projects/beta.md"),
        "projects listing did not include seeded project notes: {project_list}"
    );

    let filtered_project_list = call_tool_json(
        &client,
        "vault_list",
        json!({"path": "projects", "depth": 1, "pattern": "*.md"}),
    )
    .await?;
    let filtered_project_paths = json_result_paths(&filtered_project_list, "items")?;
    assert!(
        filtered_project_paths.contains("projects/alpha.md")
            && filtered_project_paths.contains("projects/beta.md"),
        "filtered projects listing did not include seeded markdown notes: {filtered_project_list}"
    );

    let read = call_tool_json(
        &client,
        "vault_read",
        json!({"path": "projects/alpha.md", "numbered": true}),
    )
    .await?;
    assert!(
        read["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Alpha kickoff details."),
        "read content did not contain seeded alpha text: {read}"
    );
    let alpha_content_hash = json_string_field(&read, "content_hash")?;

    let update = call_tool_json(
        &client,
        "vault_apply_unified_diff",
        json!({
            "path": "projects/alpha.md",
            "diff": "@@ -7,1 +7,1 @@\n-Alpha kickoff details.\n+Alpha kickoff details with revised milestones.",
            "expected_hash": alpha_content_hash,
        }),
    )
    .await?;
    assert_eq!(update["applied"], true, "diff should apply: {update}");

    let reread =
        call_tool_json(&client, "vault_read", json!({"path": "projects/alpha.md"})).await?;
    assert!(
        reread["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Alpha kickoff details with revised milestones."),
        "read content did not contain updated alpha text: {reread}"
    );

    let batch_read = call_tool_json(
        &client,
        "vault_batch_read",
        json!({
            "paths": [
                "projects/alpha.md",
                "projects/beta.md",
                "does/not/exist.md"
            ]
        }),
    )
    .await?;
    assert_eq!(batch_read["found"], 2);
    assert_eq!(batch_read["missing"], 1);
    let alpha_batch_entry = json_array_field(&batch_read, "files")?
        .iter()
        .find(|entry| entry["path"] == "projects/alpha.md")
        .ok_or_else(|| io::Error::other(format!("batch read missed alpha entry: {batch_read}")))?;
    assert!(
        alpha_batch_entry["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Alpha kickoff details with revised milestones."),
        "batch read alpha content did not reflect diff edit: {batch_read}"
    );

    let frontmatter_update = call_tool_json(
        &client,
        "vault_batch_frontmatter_update",
        json!({
            "updates": [
                {"path": "projects/alpha.md", "fields": {"status": "active"}},
                {"path": "projects/beta.md", "fields": {"status": "active"}}
            ]
        }),
    )
    .await?;
    for result in json_array_field(&frontmatter_update, "results")? {
        assert_eq!(
            result["updated"], true,
            "frontmatter update entry should be updated: {frontmatter_update}"
        );
    }

    let expected_active_paths = BTreeSet::from([
        "projects/alpha.md".to_string(),
        "projects/beta.md".to_string(),
    ]);
    let active_paths = wait_for_frontmatter_paths(
        &client,
        json!({
            "field": "status",
            "value": "active",
            "path_prefix": "projects/",
            "max_results": 5
        }),
        expected_active_paths.clone(),
    )
    .await?;
    assert_eq!(active_paths, expected_active_paths);

    let search = call_tool_json(
        &client,
        "vault_search",
        json!({"query": "revised milestones", "max_results": 5}),
    )
    .await?;
    let search_text = serde_json::to_string(&search)?;
    assert!(
        search_text.contains("projects/alpha.md"),
        "search result did not reference alpha note: {search_text}"
    );

    let move_result = call_tool_json(
        &client,
        "vault_move",
        json!({"source": "projects/beta.md", "destination": "archive/beta.md"}),
    )
    .await?;
    assert_eq!(move_result["moved"], true);

    let moved_old_read =
        call_tool_json(&client, "vault_read", json!({"path": "projects/beta.md"})).await?;
    assert!(
        moved_old_read.get("error").is_some(),
        "read of moved source path should return an error payload: {moved_old_read}"
    );

    let moved_new_read =
        call_tool_json(&client, "vault_read", json!({"path": "archive/beta.md"})).await?;
    assert!(
        moved_new_read["content"]
            .as_str()
            .unwrap_or_default()
            .contains("# Beta"),
        "read of moved destination path should return beta note content: {moved_new_read}"
    );

    let delete = call_tool_json(
        &client,
        "vault_delete",
        json!({"path": "projects/alpha.md", "confirm": true}),
    )
    .await?;
    assert_eq!(delete["deleted"], true);

    let deleted_read =
        call_tool_json(&client, "vault_read", json!({"path": "projects/alpha.md"})).await?;
    assert!(
        deleted_read.get("error").is_some(),
        "post-delete read should return an error payload: {deleted_read}"
    );

    client.cancel().await?;
    dump_container_diagnostics("before gateway shutdown and residue assertion").await;
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

async fn wait_for_frontmatter_paths(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>,
    arguments: Value,
    expected_paths: BTreeSet<String>,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut last_result = Value::Null;

    while Instant::now() < deadline {
        let result = call_tool_json(client, "vault_search_frontmatter", arguments.clone()).await?;
        let paths = json_result_paths(&result, "results")?;
        if paths == expected_paths && result["total"].as_u64() == Some(expected_paths.len() as u64)
        {
            return Ok(paths);
        }

        last_result = result;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    dump_frontmatter_failure_diagnostics(client).await;
    dump_container_file_diagnostics("/vault/projects/alpha.md").await;
    dump_container_diagnostics("frontmatter search failure").await;

    Err(io::Error::other(format!(
        "frontmatter search did not return expected paths {expected_paths:?}; last result: {last_result}"
    ))
    .into())
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

fn json_array_field<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a Vec<Value>, Box<dyn std::error::Error>> {
    value.get(field).and_then(Value::as_array).ok_or_else(|| {
        io::Error::other(format!("tool result missing array field {field}: {value}")).into()
    })
}

fn json_result_paths(
    value: &Value,
    field: &str,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    Ok(json_array_field(value, field)?
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .map(str::to_owned)
        .collect())
}

fn json_string_field<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    value.get(field).and_then(Value::as_str).ok_or_else(|| {
        io::Error::other(format!("tool result missing string field {field}: {value}")).into()
    })
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

async fn assert_container_running_and_vault_visible() -> Result<(), Box<dyn std::error::Error>> {
    let running = Command::new("docker")
        .args(["inspect", "--format", "{{.State.Running}}", CONTAINER_NAME])
        .output()?;
    let stdout = String::from_utf8_lossy(&running.stdout).trim().to_string();
    if !running.status.success() || stdout != "true" {
        dump_command_output(
            "docker inspect running state",
            &running,
            Some(format!("expected running=true for {CONTAINER_NAME}")),
        );
        dump_container_diagnostics("container not running during pre-test validation").await;
        return Err(format!("MCP container {CONTAINER_NAME} is not running").into());
    }

    let vault_listing = Command::new("docker")
        .args(["exec", CONTAINER_NAME, "ls", "-la", "/vault"])
        .output()?;
    dump_command_output("docker exec ls -la /vault", &vault_listing, None);
    if !vault_listing.status.success() {
        dump_container_diagnostics("vault mount pre-test validation failed").await;
        return Err("MCP container /vault mount was not visible from inside container".into());
    }

    Ok(())
}

async fn dump_frontmatter_failure_diagnostics(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>,
) {
    println!("=== E2E diagnostic: MCP vault_read projects/alpha.md ===");
    match call_tool_json(client, "vault_read", json!({"path": "projects/alpha.md"})).await {
        Ok(value) => println!("{value}"),
        Err(error) => println!("vault_read diagnostic failed: {error}"),
    }
}

async fn dump_container_file_diagnostics(path: &str) {
    let cat = Command::new("docker")
        .args(["exec", CONTAINER_NAME, "cat", path])
        .output();
    match cat {
        Ok(output) => dump_command_output(&format!("docker exec cat {path}"), &output, None),
        Err(error) => println!("docker exec cat {path} failed to start: {error}"),
    }

    let stat = Command::new("docker")
        .args(["exec", CONTAINER_NAME, "stat", path])
        .output();
    match stat {
        Ok(output) => dump_command_output(&format!("docker exec stat {path}"), &output, None),
        Err(error) => println!("docker exec stat {path} failed to start: {error}"),
    }
}

async fn dump_container_diagnostics(context: &str) {
    println!("=== E2E diagnostic: container diagnostics ({context}) ===");

    for (label, mut command) in [
        ("docker logs brain3-mcp-vault-tools", {
            let mut command = Command::new("docker");
            command.args(["logs", CONTAINER_NAME]);
            command
        }),
        ("docker inspect brain3-mcp-vault-tools", {
            let mut command = Command::new("docker");
            command.args([
                "inspect",
                "--format",
                "name={{.Name}} image={{.Config.Image}} state={{json .State}} driver={{.Driver}} mounts={{json .Mounts}} network_mode={{.HostConfig.NetworkMode}} ports={{json .NetworkSettings.Ports}}",
                CONTAINER_NAME,
            ]);
            command
        }),
        ("docker info storage driver", {
            let mut command = Command::new("docker");
            command.args(["info", "--format", "Storage Driver: {{.Driver}}"]);
            command
        }),
    ] {
        match command.output() {
            Ok(output) => dump_command_output(label, &output, None),
            Err(error) => println!("{label} failed to start: {error}"),
        }
    }
}

fn dump_command_output(label: &str, output: &std::process::Output, note: Option<String>) {
    println!("--- {label} ---");
    if let Some(note) = note {
        println!("{note}");
    }
    println!("status: {}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.is_empty() {
        println!("stdout: <empty>");
    } else {
        println!("stdout:\n{stdout}");
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.is_empty() {
        println!("stderr: <empty>");
    } else {
        println!("stderr:\n{stderr}");
    }
}
