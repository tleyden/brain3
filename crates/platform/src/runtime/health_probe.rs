use std::error::Error;
use std::time::{Duration, Instant};

const PROBE_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_MAX_ATTEMPTS: u32 = 7;

/// Calls `vault_list` directly on the MCP upstream to verify end-to-end connectivity,
/// including the shared-secret authentication header.  Returns an error string if
/// the probe fails for any reason (network, auth, JSON-RPC error, etc.).
pub async fn probe_mcp_vault_list(upstream_url: &str, upstream_secret: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
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
        timeout_ms = PROBE_TOTAL_TIMEOUT.as_millis() as u64,
        max_attempts = PROBE_MAX_ATTEMPTS,
        request_bytes = body_str.len(),
        has_upstream_secret_header = !upstream_secret.is_empty(),
        accept = "application/json, text/event-stream",
        content_type = "application/json",
        "probing MCP health via vault_list"
    );

    let started_at = Instant::now();
    let mut last_error = None;

    for attempt in 1..=PROBE_MAX_ATTEMPTS {
        let elapsed = started_at.elapsed();
        if elapsed >= PROBE_TOTAL_TIMEOUT {
            break;
        }

        let remaining_budget = PROBE_TOTAL_TIMEOUT.saturating_sub(elapsed);
        tracing::info!(
            upstream_url = %url,
            attempt,
            max_attempts = PROBE_MAX_ATTEMPTS,
            elapsed_ms = elapsed.as_millis() as u64,
            remaining_budget_ms = remaining_budget.as_millis() as u64,
            "MCP health probe attempt starting"
        );

        let attempt_started_at = Instant::now();
        match run_probe_attempt(&client, &url, upstream_secret, &body_str, remaining_budget).await {
            Ok(()) => {
                tracing::info!(
                    upstream_url = %url,
                    attempt,
                    max_attempts = PROBE_MAX_ATTEMPTS,
                    attempt_elapsed_ms = attempt_started_at.elapsed().as_millis() as u64,
                    total_elapsed_ms = started_at.elapsed().as_millis() as u64,
                    "MCP health probe attempt succeeded"
                );
                return Ok(());
            }
            Err(error) => {
                let total_elapsed = started_at.elapsed();
                let attempts_remaining = PROBE_MAX_ATTEMPTS.saturating_sub(attempt);
                let can_retry = attempts_remaining > 0 && total_elapsed < PROBE_TOTAL_TIMEOUT;

                tracing::warn!(
                    upstream_url = %url,
                    attempt,
                    max_attempts = PROBE_MAX_ATTEMPTS,
                    attempt_elapsed_ms = attempt_started_at.elapsed().as_millis() as u64,
                    total_elapsed_ms = total_elapsed.as_millis() as u64,
                    will_retry = can_retry,
                    error = %error,
                    "MCP health probe attempt failed"
                );
                last_error = Some(error);

                if !can_retry {
                    break;
                }

                let delay = retry_delay(
                    PROBE_TOTAL_TIMEOUT.saturating_sub(total_elapsed),
                    attempts_remaining,
                );
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    let total_elapsed_ms = started_at.elapsed().as_millis() as u64;
    let last_error = last_error.unwrap_or_else(|| {
        "MCP health probe did not run before timeout budget was exhausted".to_string()
    });
    Err(format!(
        "MCP health probe failed after {PROBE_MAX_ATTEMPTS} attempts over {total_elapsed_ms}ms: {last_error}"
    ))
}

async fn run_probe_attempt(
    client: &reqwest::Client,
    url: &str,
    upstream_secret: &str,
    body_str: &str,
    timeout: Duration,
) -> Result<(), String> {
    let started_at = Instant::now();
    let response = client
        .post(url)
        .timeout(timeout)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("x-brain3-upstream-secret", upstream_secret)
        .body(body_str.to_owned())
        .send()
        .await
        .map_err(|e| {
            let elapsed_ms = started_at.elapsed().as_millis() as u64;
            let detail = describe_reqwest_error(&e);
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

fn retry_delay(remaining_budget: Duration, attempts_remaining: u32) -> Duration {
    if attempts_remaining == 0 {
        return Duration::ZERO;
    }

    let slots = attempts_remaining as u128 + 1;
    let millis = (remaining_budget.as_millis() / slots) as u64;
    Duration::from_millis(millis.max(1))
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::{probe_mcp_vault_list, PROBE_TOTAL_TIMEOUT};

    #[test]
    fn mcp_health_probe_has_headroom_for_large_vault_first_rpc() {
        assert_eq!(PROBE_TOTAL_TIMEOUT, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn probe_retries_transient_transport_failures_until_success() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test probe server");
        let addr = listener.local_addr().expect("listener addr");
        let attempts_for_server = Arc::clone(&attempts);

        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.expect("accept probe request");
                let attempt = attempts_for_server.fetch_add(1, Ordering::SeqCst) + 1;
                let mut buffer = [0_u8; 1024];
                let _ = socket.read(&mut buffer).await;

                if attempt < 7 {
                    continue;
                }

                let body =
                    b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"content\":[],\"isError\":false}}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    String::from_utf8_lossy(body)
                );
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("write successful probe response");
                break;
            }
        });

        let result = probe_mcp_vault_list(&format!("http://{}", addr), "shared-secret").await;

        if server.is_finished() {
            server.await.expect("join probe server");
        } else {
            server.abort();
        }

        assert!(
            result.is_ok(),
            "expected probe to succeed after retries: {result:?}"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 7);
    }
}
