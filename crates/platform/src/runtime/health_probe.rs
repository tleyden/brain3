use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Calls `vault_list` directly on the MCP upstream to verify end-to-end connectivity,
/// including the shared-secret authentication header.  Returns an error string if
/// the probe fails for any reason (network, auth, JSON-RPC error, etc.).
pub async fn probe_mcp_vault_list(
    upstream_url: &str,
    upstream_secret: &str,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .no_proxy()
        .build()
        .map_err(|e| format!("failed to build HTTP client for MCP health probe: {e}"))?;

    let url = format!("{}/mcp", upstream_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "vault_list",
            "arguments": {}
        }
    });

    let body_str = serde_json::to_string(&body)
        .map_err(|e| format!("MCP health probe: failed to serialize request: {e}"))?;

    tracing::info!(upstream_url = %url, "probing MCP health via vault_list");

    let response = client
        .post(&url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("x-brain3-upstream-secret", upstream_secret)
        .body(body_str)
        .send()
        .await
        .map_err(|e| format!("MCP health probe request failed: {e}"))?;

    let status = response.status();

    if !status.is_success() {
        let preview = response
            .bytes()
            .await
            .unwrap_or_default()
            .iter()
            .take(256)
            .copied()
            .collect::<Vec<u8>>();
        let preview = String::from_utf8_lossy(&preview).into_owned();
        return Err(format!("MCP health probe returned HTTP {status}: {preview}"));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    if content_type.contains("text/event-stream") {
        // SSE transport: 200 OK is sufficient — we cannot easily consume the stream.
        tracing::info!("MCP health probe: server returned SSE stream on 200, treating as success");
        return Ok(());
    }

    let body_bytes = response
        .bytes()
        .await
        .map_err(|e| format!("MCP health probe: failed to read response body: {e}"))?;
    let json: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| format!("MCP health probe: failed to parse response as JSON: {e}"))?;

    if let Some(err) = json.get("error") {
        return Err(format!(
            "MCP health probe: vault_list returned JSON-RPC error: {err}"
        ));
    }

    tracing::info!("MCP health probe succeeded: vault_list returned OK");
    Ok(())
}
