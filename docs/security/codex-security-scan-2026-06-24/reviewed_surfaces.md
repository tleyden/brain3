# Reviewed Surfaces

| Surface | Risk Area | Outcome | Notes |
|---|---|---|---|
| Tunnel bootstrap and setup defaults | Public ingress by default | Reported | First-run setup and env parsing default Brain3 to a Cloudflare quick tunnel when tunneling is not explicitly disabled. |
| OAuth redirect URI binding policy | Redirect URI allowlisting | Reported | The preregistered client id is fixed, but Brain3 accepts caller-supplied redirect URIs and binds them into the authorization flow. |
| OAuth metadata and bearer challenge metadata | Host/header trust | Reported | Metadata output and 401 bearer challenges derive public URLs from request-supplied forwarded host values instead of a configured public origin. |
| Gateway and MCP logging | Sensitive data in logs | Reported | Both the gateway and the Python MCP server log full MCP bodies at trace level, and gateway temp-log permissions are not explicitly clamped after creation. |
| OAuth registration surface | Public-client or DCR expansion | Not applicable | No `/oauth/register` route or public-client token flow was present in the checked revision. |
| Named tunnel ingress config writer | Accidental proxy to unrelated localhost ports | Rejected | The checked-in example and config writer both pin ingress to the loopback gateway port and terminate with `http_status:404`. |
| Vault filesystem path controls | Path traversal / vault escape | Rejected | Vault path resolution rejects null bytes, dot-prefixed components, and paths that resolve outside the configured vault root. |
| OAuth authorization-code lifetime | Replay window / architectural code lifetime | Needs follow-up | The underlying `oxide-auth` authorizer still mints 10-minute authorization codes. This remains open, but stronger directly Brain3-owned policy issues took priority in this pass. |
| Cloudflare credential-file permissions | Local credential exposure | Needs follow-up | Named-tunnel credential lookup uses `~/.cloudflared/<id>.json` without an explicit permission check in the reviewed revision. |
| Local secret storage and token retention | Plaintext local secrets | Needs follow-up | `.env`, the upstream shared secret, and the SQLite token database remain local plaintext storage surfaces with partial mitigations but without a stronger system secret store. |
