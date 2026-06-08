# MCP Upstream Shared Secret Implementation Plan

> **For agentic workers:** Execute this plan inline and serially. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure the containerized MCP server only serves requests that came through the host OAuth2 gateway by adding one private shared-secret hop between gateway and MCP.

**Architecture:** Leave OAuth and network topology alone for now. The host keeps a single secret file, `run-container.sh` bind-mounts it into the MCP container, `start-oauth2-server.sh` points the gateway at the same host file, the gateway injects the secret on every proxied `/mcp` request, and the authless MCP server rejects any direct request missing that secret.

**Tech Stack:** Python 3.12+, Starlette, httpx, FastMCP, bash, Apple `container` CLI, Docker-compatible run script

---

## Scope

- In scope:
  - one shared secret between gateway and upstream MCP
  - automatic secret-file coordination in startup scripts
  - public API tests for direct-upstream denial and proxy header injection
  - small doc updates
- Out of scope:
  - mTLS
  - firewalling or outbound-network restrictions
  - extra public-origin or proxy-header config
  - token redesign
  - broader container hardening

## Why This Narrow Plan

- The real gap today is simple: the MCP upstream is authless and reachable on host loopback.
- A shared secret closes that gap without changing the public OAuth flow.
- It is the lowest-complexity change that gives the upstream its own access-control check.

## File Map

- Create: `poc/scripts/ensure-mcp-upstream-secret.sh`
  - Create or reuse one host-side secret file with strict permissions.
- Modify: `poc/obsidian-mcp-container/src/obsidian_mcp_server/config.py`
  - Add upstream shared-secret settings for the in-container file path and header name.
- Modify: `poc/obsidian-mcp-container/src/obsidian_mcp_server/server.py`
  - Reject `/mcp` requests that do not present the correct private header.
- Modify: `poc/obsidian-mcp-container/scripts/run-container.sh`
  - Call the shared helper, mount the secret file read-only, and set the in-container secret-file path.
- Create: `poc/obsidian-mcp-container/tests/test_upstream_access_control.py`
  - Lock the public upstream HTTP behavior.
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/config.py`
  - Add host-side secret-file settings for the gateway.
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/mcp_proxy.py`
  - Strip any client-supplied secret header and inject the gateway-owned one.
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/server.py`
  - Load the secret once at startup and fail closed if it is unavailable.
- Modify: `poc/oauth2-host-gw/scripts/start-oauth2-server.sh`
  - Call the shared helper and export the secret-file path automatically.
- Modify: `poc/oauth2-host-gw/tests/test_mcp_proxy.py`
  - Extend the public proxy contract to cover secret injection.
- Modify: `poc/obsidian-mcp-container/README.md`
- Modify: `poc/oauth2-host-gw/README.md`
  - Briefly document the new private gateway-to-upstream auth hop.

## Shared Secret Shape

- Host file path: `/private/tmp/agentzoo-mcp-upstream-secret`
- In-container file path: `/run/agentzoo/upstream_secret`
- Header name: `X-AgentZoo-Upstream-Secret`

These paths are implementation defaults for the POC. Do not add new user-facing config unless it is needed to get the feature working.

## Task 1: Lock the Public Contract with Tests

**Files:**
- Create: `poc/obsidian-mcp-container/tests/test_upstream_access_control.py`
- Modify: `poc/oauth2-host-gw/tests/test_mcp_proxy.py`

- [ ] **Step 1: Add a failing upstream test for missing secret**

Target behavior:
- `POST /mcp` without `X-AgentZoo-Upstream-Secret` returns `401`.

- [ ] **Step 2: Add a failing upstream test for wrong secret**

Target behavior:
- `POST /mcp` with the wrong secret returns `401`.

- [ ] **Step 3: Add a failing upstream test for correct secret**

Target behavior:
- `POST /mcp` with the correct secret gets past the new access-control layer.

- [ ] **Step 4: Extend the gateway proxy test**

Target behavior:
- the gateway injects `X-AgentZoo-Upstream-Secret` on the upstream request
- any client-supplied `X-AgentZoo-Upstream-Secret` is stripped and ignored

- [ ] **Step 5: Run the relevant tests to confirm they fail before implementation**

Run:
```bash
cd poc/obsidian-mcp-container && uv run python -m unittest tests/test_upstream_access_control.py -v
cd poc/oauth2-host-gw && uv run python -m unittest tests/test_mcp_proxy.py -v
```

Expected:
- FAIL until the shared-secret logic is implemented.

## Task 2: Add One Shared Secret Helper for Both Startup Paths

**Files:**
- Create: `poc/scripts/ensure-mcp-upstream-secret.sh`
- Modify: `poc/obsidian-mcp-container/scripts/run-container.sh`
- Modify: `poc/oauth2-host-gw/scripts/start-oauth2-server.sh`

- [ ] **Step 1: Create a tiny helper script**

Helper responsibilities:
- create `/private/tmp/agentzoo-mcp-upstream-secret` if it does not exist
- generate a long random value if creation is needed
- set mode `600`
- print the file path

- [ ] **Step 2: Wire the helper into the MCP container runner**

Rules:
- call the helper before constructing `run_args`
- mount the host secret file read-only into `/run/agentzoo/upstream_secret`
- set `UPSTREAM_SHARED_SECRET_FILE=/run/agentzoo/upstream_secret`

- [ ] **Step 3: Wire the helper into gateway startup**

Rules:
- call the same helper
- export `OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE` to the returned host file path
- do this automatically so the user does not need to manage another `.env` value

## Task 3: Enforce the Secret in the MCP Upstream

**Files:**
- Modify: `poc/obsidian-mcp-container/src/obsidian_mcp_server/config.py`
- Modify: `poc/obsidian-mcp-container/src/obsidian_mcp_server/server.py`

- [ ] **Step 1: Add minimal config constants**

Add only what is needed:

```python
UPSTREAM_SHARED_SECRET_FILE = os.environ.get(
    "UPSTREAM_SHARED_SECRET_FILE",
    "/run/agentzoo/upstream_secret",
)
UPSTREAM_SHARED_SECRET_HEADER = "x-agentzoo-upstream-secret"
```

- [ ] **Step 2: Load the secret at startup and fail closed**

Rules:
- read the file once
- trim whitespace
- exit with a startup error if the file is missing or empty

- [ ] **Step 3: Reject requests missing the shared secret**

Rules:
- compare with `hmac.compare_digest`
- return `401` on missing or wrong secret
- do not log the secret value

## Task 4: Inject the Secret in the Gateway Proxy

**Files:**
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/config.py`
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/mcp_proxy.py`
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/server.py`

- [ ] **Step 1: Add the one gateway-side config value**

Add only:

```python
OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE = os.environ.get(
    "OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE",
    "/private/tmp/agentzoo-mcp-upstream-secret",
)
```

- [ ] **Step 2: Load the secret once during app startup**

Rules:
- read the file once in `create_app()` / lifespan
- fail startup if the file is missing or empty
- keep the loaded secret in `app.state`

- [ ] **Step 3: Strip and replace the header during proxying**

Rules:
- add `x-agentzoo-upstream-secret` to stripped request headers
- inject the secret from `app.state` before sending the upstream request
- keep stripping `Authorization` as the proxy already does

## Task 5: Verify and Document the POC

**Files:**
- Modify: `poc/obsidian-mcp-container/README.md`
- Modify: `poc/oauth2-host-gw/README.md`

- [ ] **Step 1: Run the targeted test suites**

Run:
```bash
cd poc/obsidian-mcp-container && uv run python -m unittest tests/test_upstream_access_control.py -v
cd poc/oauth2-host-gw && uv run python -m unittest tests/test_mcp_proxy.py -v
```

Expected:
- PASS

- [ ] **Step 2: Manual verification**

Run:
```bash
curl -i http://127.0.0.1:8420/mcp
```

Expected:
- `401 Unauthorized`

Run:
```bash
curl -i \
  -H 'X-AgentZoo-Upstream-Secret: wrong-secret' \
  http://127.0.0.1:8420/mcp
```

Expected:
- `401 Unauthorized`

- [ ] **Step 3: Document the behavior briefly**

Required docs points:
- the MCP upstream remains authless from an OAuth perspective
- the host gateway is now authenticated to the upstream with a private shared secret
- direct calls to the upstream port are expected to fail
