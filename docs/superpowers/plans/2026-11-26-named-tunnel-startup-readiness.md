# Make named tunnel startup report usable access

## What happened

The staging rebuild produced a local image. Vault tools passed its startup checks, and the gateway on loopback returned HTTP 200. The public staging endpoint returned HTTP 530 with Cloudflare error 1033. During startup, `cloudflared` registered four connections and the subsequent registry lookup returned empty stdout. Brain3 interpreted that lookup failure as a missing tunnel and dropped the connector. A later registry lookup returned HTTP 503. Once the API recovered, it listed the existing staging tunnel with zero connections.

## Changes

- In the named tunnel adapter, distinguish a successful lookup that returns no matching tunnel from a failed command or invalid response. Report the command status and the relevant diagnostic on lookup failure. Use the registry lookup for diagnostics and stale-connection checks. Keep a connected connector running during a registry outage.
- Check readiness after the gateway binds. For a Brain3-managed named tunnel, give the public HTTPS health check a bounded startup window and report remote access as ready when it succeeds. Distinguish a disconnected connector, a failed public route, and a registry warning. A successful loopback health check reports gateway liveness.
- Apply the same result to CLI and TUI startup. In remote-only mode, a managed tunnel that fails readiness makes startup fail and triggers normal cleanup of the gateway, tunnel, and container. In both mode, retain the running gateway when the local MCP listener is usable and report remote access as unavailable. Preserve the lifecycle of externally managed tunnels.
- Show a final status for vault tools, local MCP, and remote MCP, including which access modes are disabled. Replace the generic local-gateway fallback with the access mode and endpoint the user can actually use.

## Verification

- Use a stand-in `cloudflared` and controlled readiness responses to cover a failed registry lookup with a working connector, a successful lookup with no matching tunnel, and a public endpoint that stays down. Check CLI exit and cleanup in remote-only mode and local MCP availability in both mode.
- Run the Rust test suite locally. Request the GitHub container E2E run through the `run-e2e-test` label when the owner wants it.
- Smoke-test with the staging vault. Check the public OAuth metadata, connect through the staging hostname, and read the fixture note through the AI host.
