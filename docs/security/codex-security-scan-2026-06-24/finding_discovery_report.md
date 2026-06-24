# Brain3 Finding Discovery Report

Scan target: `/Users/tleyden/Development/brain3`
Revision: `8d23b3c103f9da9a2d6acdce86c9ad0c8afbc93f`
Mode: repository-wide standard scan

## Discovery Summary

Validated candidate families promoted for further analysis:

1. `brain3-host-header-metadata`
   - Host header / forwarded-host injection into OAuth metadata and the bearer challenge `resource_metadata` URL.
   - Evidence: [crates/platform/src/http/oauth_handlers.rs]( /Users/tleyden/Development/brain3/crates/platform/src/http/oauth_handlers.rs ) `resolve_base_url` at lines 289-299 and `oauth_metadata` at 647-666; [crates/platform/src/http/mcp_handlers.rs]( /Users/tleyden/Development/brain3/crates/platform/src/http/mcp_handlers.rs ) `resolve_base_url` at 17-27 and `proxy_error_response` at 137-145.

2. `brain3-unallowlisted-redirect-uri`
   - The single preregistered client is allowed to bind arbitrary runtime `redirect_uri` values.
   - Evidence: [crates/platform/src/http/oauth_handlers.rs]( /Users/tleyden/Development/brain3/crates/platform/src/http/oauth_handlers.rs ) `validate_authorize_params` at 347-374 only checks non-empty `redirect_uri`; [crates/platform/src/http/registrar.rs]( /Users/tleyden/Development/brain3/crates/platform/src/http/registrar.rs ) `bound_redirect` at 32-45 round-trips the caller-supplied URI unchanged.

3. `brain3-quick-tunnel-default-public-ingress`
   - Public Cloudflare quick tunnels are still the default, so remote ingress is opt-out rather than opt-in.
   - Evidence: [crates/core/src/application/first_run_setup.rs]( /Users/tleyden/Development/brain3/crates/core/src/application/first_run_setup.rs ) sets `TunnelModeDraft::CloudflareQuick` by default at 30-40; [crates/platform/src/config/env_file.rs]( /Users/tleyden/Development/brain3/crates/platform/src/config/env_file.rs ) defaults `B3_CF_QUICK_TUNNEL` to `true` at 428-437; [.env.template]( /Users/tleyden/Development/brain3/.env.template ) documents `B3_CF_QUICK_TUNNEL=true` at 47-50.

4. `brain3-sensitive-mcp-trace-logs`
   - Trace logging records MCP request and response bodies, and gateway log files are created in the system temp directory without an explicit `0600` permission clamp.
   - Evidence: [crates/core/src/application/proxy_mcp.rs]( /Users/tleyden/Development/brain3/crates/core/src/application/proxy_mcp.rs ) logs request/response bodies at 104-106 and 132-134; [brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py]( /Users/tleyden/Development/brain3/brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py ) logs full MCP bodies at 240-250 and 256-270; [crates/platform/src/setup/system.rs]( /Users/tleyden/Development/brain3/crates/platform/src/setup/system.rs ) creates temp log files at 242-263 without a later permission hardening step.

## Resolved Or Downgraded Historical Items

- The custom Brain3 `constant_time_eq` wrapper is gone; comparisons now use `subtle::ConstantTimeEq`.
- The old in-memory auth-code cleanup finding is obsolete after the `oxide-auth` rebase.
- `GET /oauth/authorize` is still not rate-limited, but current code rejects invalid requests before showing the login form and does not process credentials on GET.
- The named Cloudflare tunnel config currently hardcodes `service: http://127.0.0.1:<port>` plus a final `http_status:404` ingress rule, so the specific "accidentally proxy an arbitrary sensitive localhost port" failure mode is not evidenced in the checked-in config writer.

## Residual Risks Not Promoted In This Pass

- Prompt injection remains mostly out of scope because the user controls the vault and the LLM, but it is still a real residual risk when the vault contains untrusted third-party content.
- `.env` secrets remain plaintext on disk, but the setup code writes that file with `0600`, so the current risk is local filesystem exposure rather than broad remote compromise.
- The current temp-file credential mechanism is an upstream shared secret header, not an OAuth bearer token. As of 2026-06-24 the template path is `/tmp/brain3-mcp-upstream-secret/upstream_secret`, while the code fallback in `EnvFileConfigAdapter` still points at `/tmp/brain3-mcp-upstream-secret`.
