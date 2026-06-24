# Plan: Change Default Ports to BRN3 Mnemonics

**Date:** 2026-06-24  
**Branch:** change_default_ports

## Port Mapping

| Role | Current | New | Notes |
|------|---------|-----|-------|
| OAuth2 gateway | 8421 | 2763 | BRN3 in decimal |
| Local MCP gateway | 8422 | 2764 | BRN3+1 |
| Container MCP server (host + container) | 8420 | 2765 | BRN3+2, same port on both sides |

## Files to Change

### Core constants (production code)

1. **`crates/core/src/domain/setup.rs`**
   - `DEFAULT_GATEWAY_PORT: u16 = 8421` → `2763`
   - `DEFAULT_CONTAINER_HOST_PORT: u16 = 8420` → `2765`
   - `DEFAULT_CONTAINER_MCP_PORT: u16 = 8420` → `2765`
   - `DEFAULT_LOCAL_MCP_PORT: u16 = 8422` → `2764`

2. **`crates/platform/src/config/env_file.rs`**
   - `const DEFAULT_LOCAL_MCP_PORT: u16 = 8422` → `2764`
   - `env_var_or("B3_OAUTH2_GATEWAY_PORT", "8421")` → `"2763"`
   - `env_var_or("B3_CONTAINER_HOST_PORT", "8420")` → `"2765"`
   - `env_var_or("B3_CONTAINER_MCP_PORT", "8420")` → `"2765"`

3. **`brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/config.py`**
   - `VAULT_MCP_PORT = int(os.environ.get("B3_VAULT_MCP_PORT", "8420"))` → `"2765"`

### Template files

4. **`.env.template`**
   - Line 1 comment: `Default: 8421` → `Default: 2763`
   - Line 2: `B3_OAUTH2_GATEWAY_PORT=8421` → `B3_OAUTH2_GATEWAY_PORT=2763`
   - Line 119 comment: `Default: 8420` → `Default: 2765`
   - Line 120: `B3_CONTAINER_HOST_PORT=8420` → `B3_CONTAINER_HOST_PORT=2765`
   - Line 121 comment: `Default: 8420` → `Default: 2765`
   - Line 122: `B3_CONTAINER_MCP_PORT=8420` → `B3_CONTAINER_MCP_PORT=2765`

5. **`brain3-mcp-vault-tools/.env.template`**
   - `B3_VAULT_MCP_PORT=8420` → `B3_VAULT_MCP_PORT=2765`

### Test files (update hardcoded assertions to match new defaults)

6. **`crates/core/src/application/first_run_setup.rs`** (test helpers only)
   - Line 326: `gateway_port: 8421` → `2763`
   - Line 338: `container_host_port: 8420` → `2765`
   - Line 339: `container_mcp_port: 8420` → `2765`

7. **`crates/core/src/application/ensure_container.rs`** (test only)
   - Line 524: `container_port: 8420` → `2765`

8. **`crates/platform/tests/setup_bootstrap.rs`**
   - All occurrences of `8421` → `2763`
   - All occurrences of `8420` → `2765`
   - All occurrences of `8422` → `2764`

9. **`crates/platform/tests/oauth_integration.rs`**
   - Lines 653, 676: `port: 8422` → `2764`

10. **`crates/platform/src/config/env_file.rs`** (inline tests)
    - Line 829: `B3_LOCAL_MCP_PORT=8422` → `B3_LOCAL_MCP_PORT=2764`
    - Lines 888, 902: `8422` → `2764`

11. **`brain3-mcp-vault-tools/tests/test_server_startup.py`**
    - All `8420` → `2765`

12. **`brain3-mcp-vault-tools/tests/test_tool_write_patch_api.py`**
    - `"B3_VAULT_MCP_PORT": "8420"` → `"2765"`

13. **`brain3-mcp-vault-tools/tests/test_upstream_access_control.py`**
    - All `8420` → `2765`

### Docs

14. **`brain3-mcp-vault-tools/README.md`**
    - Update default port references from 8420 → 2765

## Verification

After changes: `cargo test` must pass. Python tests: `cd brain3-mcp-vault-tools && python -m pytest tests/` must pass.

---

## Bearer Token via Temp Dir — Assessment

**Verdict: Not in use.** The upstream shared secret (the bearer token the gateway sends to the MCP container on each request via `x-brain3-upstream-secret`) is passed via container environment variable `B3_UPSTREAM_SHARED_SECRET`, not via a temp file. No code in the Rust codebase writes this secret to a file.

What does exist:
- **`B3_UPSTREAM_SHARED_SECRET_FILE`** — supported by the Python server (`config.py`, `server.py`) as an alternative to the env var. The Rust orchestrator does **not** use this path; it only ever sets `B3_UPSTREAM_SHARED_SECRET`. This file-based path is exercised only in Python-level tests (`test_upstream_access_control.py`) and is intended for standalone dev runs of the Python server outside of container orchestration.
- **SQLite token DB in temp dir** — `crates/platform/src/token_store/sqlite.rs:86` creates the OAuth token store in `env::temp_dir()`. This is an OAuth token database, not a bearer token file passed between processes; it's pre-existing and tracked as a needs-follow-up item in the security audit.

**No action needed on bearer-token-in-temp-dir** — the upstream secret path is env-var only from the Rust side. The `B3_UPSTREAM_SHARED_SECRET_FILE` Python fallback should remain documented but unused from the gateway orchestrator.

---

## Security Audit Updates to Suggest

### Update to Finding #4 — clarify scope of temp-dir risk

Add a note distinguishing:
- **Temp log files** (existing finding): gateway log files in `env::temp_dir()` — still open, needs `0600` permission clamp.
- **OAuth SQLite token DB in temp dir** (`token_store/sqlite.rs:86`): the `brain3-oauth-issuer-*.sqlite` file is also created in `env::temp_dir()` without an explicit permission fixup. This is a separate sub-issue that should be called out explicitly. Add it to the finding's affected lines and remediation.
- **Upstream secret is NOT in temp dir**: explicitly state that the upstream shared secret is passed gateway→container via environment variable (not a file), clearing the implied concern.

### New note — `B3_UPSTREAM_SHARED_SECRET_FILE` risk surface

The Python MCP server accepts a file path via `B3_UPSTREAM_SHARED_SECRET_FILE`. If an operator pointed this at a world-readable or temp-dir file, the secret could leak. Add a low-severity advisory note:
- Operators who use `B3_UPSTREAM_SHARED_SECRET_FILE` for direct-run deployments should ensure the file has `0600` permissions.
- The Rust gateway does not use this path, so risk is low for the standard container-managed deployment.

### New Threat Model note — ports

Update the Threat Model / Assets section to record the port assignments (2763/2764/2765), since these are now intentional and security-relevant (port selection affects firewall rules and accidental exposure on shared hosts).
