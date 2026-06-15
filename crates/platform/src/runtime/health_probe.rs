use std::error::Error;
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Calls `vault_list` directly on the MCP upstream to verify end-to-end connectivity,
/// including the shared-secret authentication header.  Returns an error string if
/// the probe fails for any reason (network, auth, JSON-RPC error, etc.).
pub async fn probe_mcp_vault_list(upstream_url: &str, upstream_secret: &str) -> Result<(), String> {
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

    tracing::info!(
        upstream_url = %url,
        timeout_ms = PROBE_TIMEOUT.as_millis() as u64,
        request_bytes = body_str.len(),
        has_upstream_secret_header = !upstream_secret.is_empty(),
        accept = "application/json, text/event-stream",
        content_type = "application/json",
        "probing MCP health via vault_list"
    );

    let started_at = Instant::now();
    let response = client
        .post(&url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("x-brain3-upstream-secret", upstream_secret)
        .body(body_str)
        .send()
        .await
        .map_err(|e| {
            let elapsed_ms = started_at.elapsed().as_millis() as u64;
            let detail = describe_reqwest_error(&e);
            tracing::error!(
                upstream_url = %url,
                elapsed_ms,
                detail = %detail,
                "MCP health probe transport failure"
            );
            format!("MCP health probe request failed after {elapsed_ms}ms: {detail}")
        })?;

    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let elapsed_ms = started_at.elapsed().as_millis() as u64;

    tracing::info!(
        upstream_url = %url,
        status = status.as_u16(),
        elapsed_ms,
        content_type = %content_type,
        "MCP health probe received upstream response"
    );

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
        tracing::warn!(
            upstream_url = %url,
            status = status.as_u16(),
            elapsed_ms,
            content_type = %content_type,
            body_preview = %preview,
            "MCP health probe returned non-success HTTP status"
        );
        return Err(format!(
            "MCP health probe returned HTTP {status} after {elapsed_ms}ms (content-type {content_type}): {preview}"
        ));
    }

    if content_type.contains("text/event-stream") {
        // SSE transport: 200 OK is sufficient — we cannot easily consume the stream.
        tracing::info!(
            upstream_url = %url,
            elapsed_ms,
            "MCP health probe: server returned SSE stream on 200, treating as success"
        );
        return Ok(());
    }

    let body_bytes = response
        .bytes()
        .await
        .map_err(|e| format!("MCP health probe: failed to read response body: {e}"))?;
    tracing::debug!(
        upstream_url = %url,
        elapsed_ms,
        response_bytes = body_bytes.len(),
        "MCP health probe read JSON response body"
    );
    let json: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| format!("MCP health probe: failed to parse response as JSON: {e}"))?;

    if let Some(err) = json.get("error") {
        return Err(format!(
            "MCP health probe: vault_list returned JSON-RPC error: {err}"
        ));
    }

    tracing::info!(
        upstream_url = %url,
        elapsed_ms,
        "MCP health probe succeeded: vault_list returned OK"
    );
    Ok(())
}

fn describe_reqwest_error(error: &reqwest::Error) -> String {
    let mut classes = Vec::new();

    if error.is_timeout() {
        classes.push("timeout");
    }
    if error.is_connect() {
        classes.push("connect");
    }
    if error.is_request() {
        classes.push("request");
    }
    if error.is_body() {
        classes.push("body");
    }
    if error.is_decode() {
        classes.push("decode");
    }
    if error.is_redirect() {
        classes.push("redirect");
    }

    let classes = if classes.is_empty() {
        "unknown".to_string()
    } else {
        classes.join(",")
    };

    let status = error
        .status()
        .map(|status| status.to_string())
        .unwrap_or_else(|| "<none>".to_string());
    let url = error
        .url()
        .map(|url| url.as_str().to_string())
        .unwrap_or_else(|| "<none>".to_string());
    let sources = error_source_chain(error);

    format!("classes={classes} status={status} url={url} error={error} sources={sources}")
}

fn error_source_chain(error: &reqwest::Error) -> String {
    let mut sources = Vec::new();
    let mut current = error.source();

    while let Some(source) = current {
        sources.push(source.to_string());
        current = source.source();
    }

    if sources.is_empty() {
        "<none>".to_string()
    } else {
        sources.join(" | ")
    }
}
