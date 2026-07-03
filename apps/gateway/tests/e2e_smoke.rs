#![cfg(feature = "e2e")]

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oauth2::basic::BasicClient;
use oauth2::reqwest;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, RedirectUrl, TokenResponse, TokenUrl,
};
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
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::TcpStream;

const OAUTH_PORT: u16 = 27630;
const LOCAL_MCP_PORT: u16 = 27640;
const CONTAINER_NAME: &str = "brain3-mcp-vault-tools";
const LOCAL_BEARER_TOKEN: &str = "e2e-test-bearer-token";
const OAUTH_CLIENT_ID: &str = "brain3-oauth2-client";
const OAUTH_CLIENT_SECRET: &str = "e2e-test-client-secret";
const OAUTH_USERNAME: &str = "e2e-test-user";
const OAUTH_PASSWORD: &str = "e2e-test-password";
const OAUTH_REDIRECT_URI: &str = "https://claude.ai/api/mcp/auth_callback";
const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(10);
const NETWORKED_E2E_TIMEOUT: Duration = Duration::from_secs(15);
const WHISPER_MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_WHISPER_MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";
const DEFAULT_WHISPER_MODEL_SHA256: &str =
    "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002";
const DEFAULT_WHISPER_MODEL_SIZE_BYTES: u64 = 147_964_211;

type E2eOAuthClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

#[derive(Clone, Copy)]
enum TunnelMode {
    Disabled,
    CloudflareQuick,
}

impl TunnelMode {
    fn quick_tunnel_env_value(self) -> &'static str {
        match self {
            Self::Disabled => "false",
            Self::CloudflareQuick => "true",
        }
    }

    fn enforce_hostname_check_env_value(self) -> &'static str {
        match self {
            Self::Disabled => "false",
            Self::CloudflareQuick => "true",
        }
    }

    fn uses_cloudflared_shim(self) -> bool {
        matches!(self, Self::Disabled)
    }
}

struct TempTestDir {
    root: PathBuf,
    vault: PathBuf,
    env_file: PathBuf,
    brain3_db: PathBuf,
    cloudflared_shim_dir: PathBuf,
    tunnel_mode: TunnelMode,
}

impl TempTestDir {
    fn create(tunnel_mode: TunnelMode) -> io::Result<Self> {
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
            tunnel_mode,
        };
        if tunnel_mode.uses_cloudflared_shim() {
            temp.write_cloudflared_shim()?;
        }
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
        self.write_env_file_with_extra(&[])
    }

    fn write_env_file_with_extra(&self, extra: &[(&str, String)]) -> io::Result<()> {
        let mut env_file = format!(
            "B3_OAUTH2_GATEWAY_PORT={OAUTH_PORT}\n\
             B3_OAUTH2_GATEWAY_CLIENT_ID={OAUTH_CLIENT_ID}\n\
             B3_OAUTH2_GATEWAY_CLIENT_SECRET=e2e-test-client-secret\n\
             B3_USERNAME=e2e-test-user\n\
             B3_PASSWORD=e2e-test-password\n\
             B3_TOKEN_DB_PATH={}\n\
             B3_CF_QUICK_TUNNEL={}\n\
             B3_CONTAINER_RUNTIME=docker\n\
             B3_VAULT_PATH={}\n\
             B3_CONTAINER_IMAGE_REPO=brain3-mcp-vault-tools\n\
             B3_CONTAINER_IMAGE_TAG=e2e-local\n\
             B3_UPSTREAM_SHARED_SECRET=e2e-test-upstream-secret\n\
             B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false\n\
             B3_LOCAL_MCP_PORT={LOCAL_MCP_PORT}\n\
             LOCAL_GATEWAY_MCP_BEARER_TOKEN={LOCAL_BEARER_TOKEN}\n\
             B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK={}\n\
             BRAIN3_ENABLE_SYNC_REINDEX_TOOL=true\n",
            self.brain3_db.display(),
            self.tunnel_mode.quick_tunnel_env_value(),
            self.vault.display(),
            self.tunnel_mode.enforce_hostname_check_env_value(),
        );
        for (key, value) in extra {
            env_file.push_str(key);
            env_file.push('=');
            env_file.push_str(value);
            env_file.push('\n');
        }
        fs::write(&self.env_file, env_file)
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

#[test]
fn recognizes_container_diagnostics_end_sentinel_line() {
    assert!(is_container_diagnostics_end_sentinel(
        "=== end brain3 container diagnostics: brain3-mcp-vault-tools ==="
    ));
    assert!(!is_container_diagnostics_end_sentinel(
        "=== brain3 container diagnostics: brain3-mcp-vault-tools ==="
    ));
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn container_diagnostics_end_sentinel() -> String {
    format!("=== end brain3 container diagnostics: {CONTAINER_NAME} ===")
}

fn is_container_diagnostics_end_sentinel(line: &str) -> bool {
    line == container_diagnostics_end_sentinel()
}

fn spawn_stdout_reader(stdout: ChildStdout) -> (Receiver<()>, JoinHandle<()>) {
    let (diagnostics_done_tx, diagnostics_done_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    println!("{line}");
                    if is_container_diagnostics_end_sentinel(&line) {
                        let _ = diagnostics_done_tx.send(());
                    }
                }
                Err(error) => {
                    println!("brain3 stdout reader failed: {error}");
                    break;
                }
            }
        }
    });

    (diagnostics_done_rx, handle)
}

struct Brain3Process {
    child: Child,
    diagnostics_done: Receiver<()>,
    stdout_reader: Option<JoinHandle<()>>,
}

impl Brain3Process {
    async fn spawn(
        temp: &TempTestDir,
        tunnel_mode: TunnelMode,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::spawn_with_health_port(temp, tunnel_mode, OAUTH_PORT).await
    }

    async fn spawn_local_only(temp: &TempTestDir) -> Result<Self, Box<dyn std::error::Error>> {
        Self::spawn_with_health_port(temp, TunnelMode::Disabled, LOCAL_MCP_PORT).await
    }

    async fn spawn_with_health_port(
        temp: &TempTestDir,
        tunnel_mode: TunnelMode,
        health_port: u16,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let binary = env!("CARGO_BIN_EXE_brain3");
        let mut command = Command::new(binary);
        command
            .arg("--cli")
            .arg("--env-file")
            .arg(&temp.env_file)
            .arg("--brain3-home")
            .arg(&temp.root)
            .arg("--log-level")
            .arg("debug")
            .env("B3_HOME", &temp.root)
            .env("B3_OAUTH2_GATEWAY_PORT", OAUTH_PORT.to_string())
            .env("B3_OAUTH2_GATEWAY_CLIENT_ID", OAUTH_CLIENT_ID)
            .env("B3_OAUTH2_GATEWAY_CLIENT_SECRET", OAUTH_CLIENT_SECRET)
            .env("B3_USERNAME", OAUTH_USERNAME)
            .env("B3_PASSWORD", OAUTH_PASSWORD)
            .env("B3_TOKEN_DB_PATH", &temp.brain3_db)
            .env("B3_CF_QUICK_TUNNEL", tunnel_mode.quick_tunnel_env_value())
            .env("B3_CONTAINER_RUNTIME", "docker")
            .env("B3_VAULT_PATH", &temp.vault)
            .env("B3_CONTAINER_IMAGE_REPO", "brain3-mcp-vault-tools")
            .env("B3_CONTAINER_IMAGE_TAG", "e2e-local")
            .env("B3_UPSTREAM_SHARED_SECRET", "e2e-test-upstream-secret")
            .env("B3_CONTAINER_INTERNAL_NETWORK_ISOLATION", "false")
            .env("B3_LOCAL_MCP_PORT", LOCAL_MCP_PORT.to_string())
            .env("LOCAL_GATEWAY_MCP_BEARER_TOKEN", LOCAL_BEARER_TOKEN)
            .env(
                "B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK",
                tunnel_mode.enforce_hostname_check_env_value(),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        if tunnel_mode.uses_cloudflared_shim() {
            command.env("PATH", temp.path_with_shim());
        }

        let mut child = command.spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or("brain3 child stdout was not piped")?;
        let (diagnostics_done, stdout_reader) = spawn_stdout_reader(stdout);
        let process = Self {
            child,
            diagnostics_done,
            stdout_reader: Some(stdout_reader),
        };
        process.wait_for_health(health_port).await?;
        Ok(process)
    }

    async fn wait_for_health(&self, port: u16) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last_error = String::from("health endpoint was not probed");

        while Instant::now() < deadline {
            match probe_health(port).await {
                Ok(()) => return Ok(()),
                Err(error) => last_error = error.to_string(),
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        Err(
            format!("gateway did not become healthy on port {port} within 30s: {last_error}")
                .into(),
        )
    }

    fn dump_diagnostics(&self) {
        let pid = self.child.id().to_string();
        match Command::new("kill").arg("-USR1").arg(&pid).status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                println!(
                    "failed to send SIGUSR1 diagnostics request to brain3 pid {pid}: {status}"
                );
                return;
            }
            Err(error) => {
                println!("failed to start kill for SIGUSR1 diagnostics request to brain3 pid {pid}: {error}");
                return;
            }
        }

        match self.diagnostics_done.recv_timeout(DIAGNOSTICS_TIMEOUT) {
            Ok(()) => {}
            Err(error) => {
                println!(
                    "timed out waiting for brain3 container diagnostics sentinel after SIGUSR1: {error}"
                );
            }
        }
    }

    fn join_stdout_reader(&mut self) {
        if let Some(stdout_reader) = self.stdout_reader.take() {
            let _ = stdout_reader.join();
        }
    }
}

fn real_cloudflared_on_path() -> bool {
    Command::new("cloudflared")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn read_public_tunnel_url(temp: &TempTestDir) -> Result<String, Box<dyn std::error::Error>> {
    let log_path = temp.root.join("brain3.log");
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        match fs::read_to_string(&log_path) {
            Ok(log) => {
                if let Some(url) = log.lines().find_map(extract_trycloudflare_url) {
                    return Ok(url);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err(format!(
        "did not find trycloudflare.com URL in log file within 5s: {}",
        log_path.display()
    )
    .into())
}

fn extract_trycloudflare_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|')
        .unwrap_or(rest.len());
    let url = &rest[..end];
    url.contains(".trycloudflare.com").then(|| url.to_string())
}

struct DiagnosticsDumpGuard<'a> {
    gateway: &'a Brain3Process,
}

impl<'a> DiagnosticsDumpGuard<'a> {
    fn new(gateway: &'a Brain3Process) -> Self {
        Self { gateway }
    }
}

impl Drop for DiagnosticsDumpGuard<'_> {
    fn drop(&mut self) {
        self.gateway.dump_diagnostics();
    }
}

async fn probe_health(port: u16) -> io::Result<()> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
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
            self.join_stdout_reader();
            return;
        }

        let pid = self.child.id().to_string();
        let _ = Command::new("kill").arg("-INT").arg(&pid).status();

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                self.join_stdout_reader();
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
        self.join_stdout_reader();
    }
}

#[tokio::test]
async fn e2e_smoke_1_local_docker() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempTestDir::create(TunnelMode::Disabled)?;
    temp.write_env_file()?;

    {
        let gateway = Brain3Process::spawn(&temp, TunnelMode::Disabled).await?;
        let _diagnostics_guard = DiagnosticsDumpGuard::new(&gateway);
        assert_container_running_and_vault_visible(&gateway).await?;
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
            "vault_reindex_frontmatter_sync",
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
            .ok_or_else(|| {
                io::Error::other(format!("batch read missed alpha entry: {batch_read}"))
            })?;
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

        // Synchronously rebuild the frontmatter index (needed when async file watcher is disabled)
        let reindex_result =
            call_tool_json(&client, "vault_reindex_frontmatter_sync", json!({})).await?;
        assert_eq!(reindex_result["reindexed"], true);
        assert!(
            reindex_result["file_count"].as_u64().unwrap_or_default() >= 2,
            "reindex should have found at least 2 files: {reindex_result}"
        );

        let expected_active_paths = BTreeSet::from([
            "projects/alpha.md".to_string(),
            "projects/beta.md".to_string(),
        ]);
        let active_search = call_tool_json(
            &client,
            "vault_search_frontmatter",
            json!({
                "field": "status",
                "value": "active",
                "path_prefix": "projects/",
                "max_results": 5
            }),
        )
        .await?;
        let active_paths = json_result_paths(&active_search, "results")?;
        assert_eq!(
            active_paths, expected_active_paths,
            "frontmatter search should find active project files after reindex"
        );

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
    }

    assert_no_container_residue().await?;
    Ok(())
}

#[tokio::test]
async fn e2e_smoke_4_local_mcp_transcribes_tts_audio() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempTestDir::create(TunnelMode::Disabled)?;
    ensure_default_whisper_model(&temp).await?;

    let test_audio = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/test_tts.wav")
        .canonicalize()?;
    let download_url = serve_test_audio_once(test_audio).await?;

    temp.write_env_file_with_extra(&[
        ("B3_ACCESS_MODE", "local".to_string()),
        ("B3_NATIVE_AUDIO_TRANSCRIPTION_ENABLED", "true".to_string()),
        ("B3_WHISPER_MAX_AUDIO_BYTES", "10485760".to_string()),
    ])?;

    {
        let gateway = Brain3Process::spawn_local_only(&temp).await?;
        let _diagnostics_guard = DiagnosticsDumpGuard::new(&gateway);
        let client = connect_local_mcp().await?;

        let transcript = call_tool_text(
            &client,
            "transcribe_audio_file",
            json!({
                "audio_file": {
                    "download_url": download_url,
                    "file_id": "test_tts",
                    "mime_type": "audio/wav",
                    "file_name": "test_tts.wav"
                }
            }),
        )
        .await?;
        let normalized = normalize_transcript(&transcript);
        assert_eq!(
            normalized, "hello world",
            "actual transcript: {transcript:?}"
        );

        client.cancel().await?;
    }

    assert_no_container_residue().await?;
    Ok(())
}

#[tokio::test]
async fn e2e_smoke_2_oauth_public_flow() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempTestDir::create(TunnelMode::Disabled)?;
    temp.write_env_file()?;

    {
        let gateway = Brain3Process::spawn(&temp, TunnelMode::Disabled).await?;
        let _diagnostics_guard = DiagnosticsDumpGuard::new(&gateway);
        assert_container_running_and_vault_visible(&gateway).await?;

        let http_client = oauth_http_client()?;
        let base_url = format!("http://127.0.0.1:{OAUTH_PORT}");

        assert_oauth_metadata(&http_client, &base_url).await?;
        assert_public_mcp_rejects_missing_and_invalid_bearers(&http_client, &base_url).await?;
        assert_token_rejects_wrong_client_secret(&http_client, &base_url).await?;
        assert_authorize_rejects_unregistered_client(&http_client, &base_url).await?;

        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let csrf = CsrfToken::new_random();
        assert_authorize_form_renders(&http_client, &base_url, challenge.as_str(), csrf.secret())
            .await?;

        let (mismatched_challenge, _) = PkceCodeChallenge::new_random_sha256();
        let mismatched_state = CsrfToken::new_random();
        let mismatched_code = submit_login_for_authorization_code(
            &http_client,
            &base_url,
            mismatched_challenge.as_str(),
            mismatched_state.secret(),
        )
        .await?;
        assert_token_rejects_mismatched_pkce_verifier(&base_url, mismatched_code).await?;

        let code = submit_login_for_authorization_code(
            &http_client,
            &base_url,
            challenge.as_str(),
            csrf.secret(),
        )
        .await?;

        let token_response = oauth_client(&base_url)?
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(verifier)
            .request_async(&http_client)
            .await?;
        assert!(
            token_response.refresh_token().is_some(),
            "OAuth token response should include a refresh token"
        );
        let access_token = token_response.access_token().secret();

        let client = connect_public_mcp(&base_url, access_token).await?;
        let create = call_tool_json(
            &client,
            "vault_create_overwrite_file",
            json!({
                "path": "oauth/public-flow.md",
                "content": "# OAuth public flow\nReached through the OAuth-protected public MCP path.\n",
            }),
        )
        .await?;
        assert_eq!(create["path"], "oauth/public-flow.md");
        assert_eq!(create["created"], true);

        let read = call_tool_json(
            &client,
            "vault_read",
            json!({"path": "oauth/public-flow.md"}),
        )
        .await?;
        assert!(
            read["content"]
                .as_str()
                .unwrap_or_default()
                .contains("OAuth-protected public MCP path"),
            "public OAuth MCP read did not return the created note: {read}"
        );

        client.cancel().await?;
    }

    assert_no_container_residue().await?;
    Ok(())
}

#[tokio::test]
async fn e2e_smoke_3_oauth_quick_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    if !real_cloudflared_on_path() {
        println!("SKIP: e2e_smoke_3_oauth_quick_tunnel requires cloudflared on PATH");
        return Ok(());
    }

    let temp = TempTestDir::create(TunnelMode::CloudflareQuick)?;
    temp.write_env_file()?;

    {
        let gateway = Brain3Process::spawn(&temp, TunnelMode::CloudflareQuick).await?;
        let _diagnostics_guard = DiagnosticsDumpGuard::new(&gateway);
        assert_container_running_and_vault_visible(&gateway).await?;

        let http_client = oauth_http_client()?;
        let base_url = read_public_tunnel_url(&temp).await?;
        println!("quick tunnel public URL: {base_url}");

        wait_for_public_tunnel_health(&http_client, &base_url).await?;
        assert_public_mcp_rejects_missing_and_invalid_bearers(&http_client, &base_url).await?;

        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let csrf = CsrfToken::new_random();
        let code = submit_login_for_authorization_code(
            &http_client,
            &base_url,
            challenge.as_str(),
            csrf.secret(),
        )
        .await?;

        let token_response = oauth_client(&base_url)?
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(verifier)
            .request_async(&http_client)
            .await?;
        let access_token = token_response.access_token().secret();

        let create = post_public_mcp_tool_call_json(
            &http_client,
            &base_url,
            access_token,
            "vault_create_overwrite_file",
            json!({
                "path": "oauth/quick-tunnel.md",
                "content": "# OAuth quick tunnel\nReached through a Cloudflare quick tunnel.\n",
            }),
        )
        .await?;
        assert_eq!(create["path"], "oauth/quick-tunnel.md");
        assert_eq!(create["created"], true);

        let read = post_public_mcp_tool_call_json(
            &http_client,
            &base_url,
            access_token,
            "vault_read",
            json!({"path": "oauth/quick-tunnel.md"}),
        )
        .await?;
        assert!(
            read["content"]
                .as_str()
                .unwrap_or_default()
                .contains("Cloudflare quick tunnel"),
            "quick tunnel MCP read did not return the created note: {read}"
        );
    }

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

async fn connect_public_mcp(
    base_url: &str,
    access_token: &str,
) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>, Box<dyn std::error::Error>>
{
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("{base_url}/mcp"))
            .auth_header(access_token),
    );
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("brain3-e2e-smoke", "0.0.0"),
    );

    Ok(
        tokio::time::timeout(NETWORKED_E2E_TIMEOUT, client_info.serve(transport))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "timed out connecting public MCP")
            })??,
    )
}

async fn serve_test_audio_once(path: PathBuf) -> Result<String, Box<dyn std::error::Error>> {
    let body = fs::read(path)?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut request = vec![0; 1024];
        let _ = stream.read(&mut request).await;
        let headers = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: audio/wav\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(headers.as_bytes()).await;
        let _ = stream.write_all(&body).await;
        let _ = stream.shutdown().await;
    });
    Ok(format!("http://{addr}/test_tts.wav"))
}

async fn ensure_default_whisper_model(
    temp: &TempTestDir,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let model_dir = temp.root.join("whisper-models");
    let model_path = model_dir.join("ggml-base.en.bin");
    fs::create_dir_all(&model_dir)?;

    if model_path.is_file() {
        verify_default_whisper_model(&model_path)?;
        return Ok(model_path);
    }

    let partial_path = model_dir.join("ggml-base.en.bin.download");
    let _ = fs::remove_file(&partial_path);

    println!(
        "downloading default Whisper model to {}",
        model_path.display()
    );
    let client = whisper_model_http_client()?;
    let mut response = client.get(DEFAULT_WHISPER_MODEL_URL).send().await?;
    if !response.status().is_success() {
        return Err(format!(
            "failed to download default Whisper model: HTTP {}",
            response.status()
        )
        .into());
    }

    let expected_size = response
        .content_length()
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_WHISPER_MODEL_SIZE_BYTES);
    if expected_size != DEFAULT_WHISPER_MODEL_SIZE_BYTES {
        return Err(format!(
            "default Whisper model size changed: expected {DEFAULT_WHISPER_MODEL_SIZE_BYTES}, got {expected_size}"
        )
        .into());
    }

    let mut file = fs::File::create(&partial_path)?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0_u64;

    while let Some(chunk) = response.chunk().await? {
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or("default Whisper model download byte count overflowed")?;
        if downloaded > DEFAULT_WHISPER_MODEL_SIZE_BYTES {
            return Err(format!(
                "default Whisper model exceeded expected size: {downloaded} > {DEFAULT_WHISPER_MODEL_SIZE_BYTES}"
            )
            .into());
        }
        hasher.update(&chunk);
        std::io::Write::write_all(&mut file, &chunk)?;
    }
    std::io::Write::flush(&mut file)?;

    if downloaded != DEFAULT_WHISPER_MODEL_SIZE_BYTES {
        return Err(format!(
            "default Whisper model download size mismatch: expected {DEFAULT_WHISPER_MODEL_SIZE_BYTES}, got {downloaded}"
        )
        .into());
    }

    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != DEFAULT_WHISPER_MODEL_SHA256 {
        return Err(format!(
            "default Whisper model SHA-256 mismatch: expected {DEFAULT_WHISPER_MODEL_SHA256}, got {actual_hash}"
        )
        .into());
    }

    fs::rename(&partial_path, &model_path)?;
    Ok(model_path)
}

fn verify_default_whisper_model(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes = 0_u64;

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or("default Whisper model byte count overflowed")?;
        hasher.update(&buffer[..read]);
    }

    if bytes != DEFAULT_WHISPER_MODEL_SIZE_BYTES {
        return Err(format!(
            "existing default Whisper model size mismatch at {}: expected {DEFAULT_WHISPER_MODEL_SIZE_BYTES}, got {bytes}",
            path.display()
        )
        .into());
    }

    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != DEFAULT_WHISPER_MODEL_SHA256 {
        return Err(format!(
            "existing default Whisper model SHA-256 mismatch at {}: expected {DEFAULT_WHISPER_MODEL_SHA256}, got {actual_hash}",
            path.display()
        )
        .into());
    }

    Ok(())
}

fn whisper_model_http_client() -> Result<reqwest::Client, Box<dyn std::error::Error>> {
    Ok(reqwest::Client::builder()
        .timeout(WHISPER_MODEL_DOWNLOAD_TIMEOUT)
        .build()?)
}

fn oauth_http_client() -> Result<reqwest::Client, Box<dyn std::error::Error>> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(NETWORKED_E2E_TIMEOUT)
        .build()?)
}

async fn wait_for_public_tunnel_health(
    http_client: &reqwest::Client,
    base_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut last_error = String::from("public health endpoint was not probed");

    while Instant::now() < deadline {
        match http_client
            .get(format!("{base_url}/health"))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) if response.status() == reqwest::StatusCode::OK => return Ok(()),
            Ok(response) => last_error = format!("status {}", response.status()),
            Err(error) => last_error = format!("{error:?}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Err(format!(
        "public quick tunnel health did not become reachable within 240s at {base_url}: {last_error}"
    )
    .into())
}

fn oauth_client(base_url: &str) -> Result<E2eOAuthClient, Box<dyn std::error::Error>> {
    Ok(BasicClient::new(ClientId::new(OAUTH_CLIENT_ID.to_string()))
        .set_client_secret(ClientSecret::new(OAUTH_CLIENT_SECRET.to_string()))
        .set_auth_uri(AuthUrl::new(format!("{base_url}/oauth/authorize"))?)
        .set_token_uri(TokenUrl::new(format!("{base_url}/oauth/token"))?)
        .set_redirect_uri(RedirectUrl::new(OAUTH_REDIRECT_URI.to_string())?)
        .set_auth_type(AuthType::RequestBody))
}

async fn assert_oauth_metadata(
    http_client: &reqwest::Client,
    base_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = http_client
        .get(format!("{base_url}/.well-known/oauth-authorization-server"))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = serde_json::from_str(&response.text().await?)?;
    assert_eq!(
        body["authorization_endpoint"],
        format!("{base_url}/oauth/authorize")
    );
    assert_eq!(body["token_endpoint"], format!("{base_url}/oauth/token"));
    assert_eq!(body["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(
        body["token_endpoint_auth_methods_supported"],
        json!(["client_secret_post"])
    );
    Ok(())
}

async fn assert_authorize_form_renders(
    http_client: &reqwest::Client,
    base_url: &str,
    code_challenge: &str,
    state: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = http_client
        .get(format!("{base_url}/oauth/authorize"))
        .query(&[
            ("response_type", "code"),
            ("client_id", OAUTH_CLIENT_ID),
            ("redirect_uri", OAUTH_REDIRECT_URI),
            ("state", state),
            ("code_challenge", code_challenge),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.text().await?;
    assert!(
        body.contains("<form") && body.contains("method=\"post\""),
        "authorize GET should render the login form HTML: {body}"
    );
    Ok(())
}

async fn submit_login_for_authorization_code(
    http_client: &reqwest::Client,
    base_url: &str,
    code_challenge: &str,
    state: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let response = http_client
        .post(format!("{base_url}/oauth/authorize"))
        .form(&[
            ("response_type", "code"),
            ("client_id", OAUTH_CLIENT_ID),
            ("redirect_uri", OAUTH_REDIRECT_URI),
            ("state", state),
            ("code_challenge", code_challenge),
            ("code_challenge_method", "S256"),
            ("username", OAUTH_USERNAME),
            ("password", OAUTH_PASSWORD),
        ])
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::FOUND);

    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or("authorize POST did not include Location header")?;
    let redirect = reqwest::Url::parse(location)?;
    assert_eq!(
        redirect.as_str().split('?').next().unwrap_or_default(),
        OAUTH_REDIRECT_URI
    );
    assert_eq!(query_param(&redirect, "state").as_deref(), Some(state));
    query_param(&redirect, "code").ok_or_else(|| {
        io::Error::other(format!(
            "authorize redirect did not include code: {location}"
        ))
        .into()
    })
}

async fn assert_public_mcp_rejects_missing_and_invalid_bearers(
    http_client: &reqwest::Client,
    base_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let no_bearer = post_mcp_tools_list(http_client, base_url, None).await?;
    assert_eq!(no_bearer.status(), reqwest::StatusCode::UNAUTHORIZED);
    let no_bearer_auth = header_value(&no_bearer, reqwest::header::WWW_AUTHENTICATE)?;
    assert!(
        no_bearer_auth.contains(".well-known/oauth-protected-resource/mcp"),
        "401 should include protected-resource metadata hint: {no_bearer_auth}"
    );

    let garbage_bearer = post_mcp_tools_list(http_client, base_url, Some("garbage-token")).await?;
    assert_eq!(garbage_bearer.status(), reqwest::StatusCode::UNAUTHORIZED);
    Ok(())
}

async fn post_mcp_tools_list(
    http_client: &reqwest::Client,
    base_url: &str,
    bearer_token: Option<&str>,
) -> Result<reqwest::Response, Box<dyn std::error::Error>> {
    let mut request = http_client
        .post(format!("{base_url}/mcp"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))?);
    if let Some(token) = bearer_token {
        request = request.bearer_auth(token);
    }
    Ok(request.send().await?)
}

async fn post_public_mcp_tool_call_json(
    http_client: &reqwest::Client,
    base_url: &str,
    bearer_token: &str,
    name: &'static str,
    arguments: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let response = http_client
        .post(format!("{base_url}/mcp"))
        .bearer_auth(bearer_token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .body(serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
            }
        }))?)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;
    assert!(
        status.is_success(),
        "public MCP tool call {name} returned HTTP {status}: {body}"
    );

    let value: Value = serde_json::from_str(&body)?;
    assert!(
        value.get("error").is_none(),
        "public MCP tool call {name} returned JSON-RPC error: {value}"
    );
    tool_result_value_json(&value)
}

async fn assert_token_rejects_wrong_client_secret(
    http_client: &reqwest::Client,
    base_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = http_client
        .post(format!("{base_url}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", "not-a-real-code"),
            ("redirect_uri", OAUTH_REDIRECT_URI),
            ("client_id", OAUTH_CLIENT_ID),
            ("client_secret", "wrong-secret"),
            ("code_verifier", "not-a-real-verifier"),
        ])
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: Value = serde_json::from_str(&response.text().await?)?;
    assert_eq!(body["error"], "invalid_client");
    Ok(())
}

async fn assert_authorize_rejects_unregistered_client(
    http_client: &reqwest::Client,
    base_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (challenge, _) = PkceCodeChallenge::new_random_sha256();
    let response = http_client
        .get(format!("{base_url}/oauth/authorize"))
        .query(&[
            ("response_type", "code"),
            ("client_id", "unregistered-client"),
            ("redirect_uri", OAUTH_REDIRECT_URI),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: Value = serde_json::from_str(&response.text().await?)?;
    assert_eq!(body["error"], "invalid_client");
    Ok(())
}

async fn assert_token_rejects_mismatched_pkce_verifier(
    base_url: &str,
    code: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let (_, wrong_verifier) = PkceCodeChallenge::new_random_sha256();
    let http_client = oauth_http_client()?;
    let token_result = oauth_client(base_url)?
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(wrong_verifier)
        .request_async(&http_client)
        .await;
    let error = token_result.expect_err("token exchange with wrong PKCE verifier should fail");
    assert!(
        error.to_string().contains("invalid_grant"),
        "wrong PKCE verifier should be rejected as invalid_grant: {error}"
    );
    Ok(())
}

fn query_param(url: &reqwest::Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(key, value)| (key == name).then(|| value.into_owned()))
}

fn header_value(
    response: &reqwest::Response,
    header: reqwest::header::HeaderName,
) -> Result<&str, Box<dyn std::error::Error>> {
    response
        .headers()
        .get(header)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| io::Error::other("response missing expected header").into())
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
    let result = client.call_tool(CallToolRequestParams::new(name).with_arguments(arguments));
    let result = tokio::time::timeout(NETWORKED_E2E_TIMEOUT, result)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out calling MCP tool"))??;
    assert!(
        result.is_error != Some(true),
        "tool {name} returned MCP error result: {result:?}"
    );
    Ok(tool_result_json(&result)?)
}

async fn call_tool_text(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>,
    name: &'static str,
    arguments: Value,
) -> Result<String, Box<dyn std::error::Error>> {
    let arguments = arguments
        .as_object()
        .cloned()
        .ok_or("tool arguments must be a JSON object")?;
    let result = client.call_tool(CallToolRequestParams::new(name).with_arguments(arguments));
    let result = tokio::time::timeout(NETWORKED_E2E_TIMEOUT, result)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out calling MCP tool"))??;
    assert!(
        result.is_error != Some(true),
        "tool {name} returned MCP error result: {result:?}"
    );
    result
        .content
        .iter()
        .find_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.to_string()),
            _ => None,
        })
        .ok_or_else(|| io::Error::other("tool result did not include text content").into())
}

fn normalize_transcript(transcript: &str) -> String {
    transcript
        .trim()
        .trim_matches(|ch: char| ch.is_ascii_punctuation() || ch.is_whitespace())
        .to_ascii_lowercase()
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

fn tool_result_value_json(value: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let text = value
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| {
            content.iter().find_map(|block| {
                (block.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(Value::as_str))
                    .flatten()
            })
        })
        .ok_or_else(|| {
            io::Error::other(format!("tool result did not include text content: {value}"))
        })?;
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

async fn assert_container_running_and_vault_visible(
    gateway: &Brain3Process,
) -> Result<(), Box<dyn std::error::Error>> {
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
        gateway.dump_diagnostics();
        return Err(format!("MCP container {CONTAINER_NAME} is not running").into());
    }

    let vault_listing = Command::new("docker")
        .args(["exec", CONTAINER_NAME, "ls", "-la", "/vault"])
        .output()?;
    dump_command_output("docker exec ls -la /vault", &vault_listing, None);
    if !vault_listing.status.success() {
        gateway.dump_diagnostics();
        return Err("MCP container /vault mount was not visible from inside container".into());
    }

    Ok(())
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
