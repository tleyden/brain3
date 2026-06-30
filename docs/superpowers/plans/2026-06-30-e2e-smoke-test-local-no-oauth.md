# E2E Smoke Test 1: Local MCP, No OAuth

## Goal

An automated end-to-end test that:
1. Builds the `brain3-mcp-vault-tools` container image from local source (not a registry pull)
2. Starts the Brain3 gateway binary against that container
3. Exercises the full CRUDS loop over the local MCP endpoint (no OAuth)
4. Shuts down cleanly and verifies no container residue
5. Runs in CI on every release tag (`v*`)

---

## Architecture context

The **local MCP path** (no OAuth) is what this test covers:

```
Test (rmcp client)
  --[Bearer token]--> brain3 gateway port 27640 (/mcp)
    --[upstream secret]--> brain3-mcp-vault-tools container port 2765 (/mcp)
```

Relevant config vars:
- `LOCAL_GATEWAY_MCP_BEARER_TOKEN` + `B3_LOCAL_MCP_PORT` (27640) — enables the local MCP listener
- `B3_UPSTREAM_SHARED_SECRET` — shared secret between gateway and container
- `B3_CONTAINER_RUNTIME=docker`, `B3_VAULT_PATH`, `B3_CONTAINER_IMAGE_REPO` — controls which container is started

---

## Decision: why the image must be built from source

Using a registry-pulled image means a bug introduced in `brain3-mcp-vault-tools/` could exist in the running container while the test passes against old code. The E2E test only has value if it exercises code at HEAD. Therefore:

- **CI builds the image from source** before running the test, using the same `docker build` command already in `container.yml`
- The test's `.env` points to that locally-built image tag (`e2e-local`)
- For local dev, the README documents the same `docker build` step

---

## Decision: `#[ignore]` vs. Cargo feature flag

Two common ways to make tests opt-in in Rust:

**`#[ignore]`** marks a test as skipped by default. You run skipped tests with:
```
cargo test -p brain3 --test e2e_smoke -- --include-ignored
```
The test code is always compiled into the binary; it's only skipped at runtime.

**Cargo feature flag** gates the test file itself with `#[cfg(feature = "e2e")]`. You enable it with:
```
cargo test -p brain3 --test e2e_smoke --features e2e
```
The test doesn't compile at all in a normal build.

**Recommendation: Cargo feature flag (`--features e2e`).** Reasons:
- E2E tests will eventually pull in test-only dependencies (e.g. helpers, process management). A feature flag keeps those out of the normal build.
- The mental model matches the user's expectation: pass a flag to opt in.
- `cargo test` stays completely clean — no need to know about `--include-ignored`.
- CI is explicit: the workflow step that runs E2E tests is obviously different from the step that runs unit tests.

The flag is named `e2e` and declared in `apps/gateway/Cargo.toml` under `[features]`.

---

## Test structure

### Location

`apps/gateway/tests/e2e_smoke.rs`, gated with `#[cfg(feature = "e2e")]` at the top of the file.

**Why here:** Cargo sets `env!("CARGO_BIN_EXE_brain3")` in integration tests of the `brain3` crate, giving us the exact path of the built binary without any discovery logic.

### Helper types (in the same file or a `tests/e2e_helpers/` mod)

#### `TempTestDir`
- Creates `/tmp/brain3-e2e-<unique>/`
- Sub-paths: `vault/`, `brain3.db`, `brain3.log`
- `Drop` removes the directory

#### `TestEnv`
Generates and writes a `.env` with known test values:

```
B3_OAUTH2_GATEWAY_PORT=27630
B3_OAUTH2_GATEWAY_CLIENT_SECRET=e2e-test-client-secret
B3_USERNAME=e2e-test-user
B3_PASSWORD=e2e-test-password
B3_TOKEN_DB_PATH=<temp>/brain3.db
B3_CF_QUICK_TUNNEL=false
B3_CONTAINER_RUNTIME=docker
B3_VAULT_PATH=<temp>/vault
B3_CONTAINER_IMAGE_REPO=brain3-mcp-vault-tools    # local image, no registry prefix
B3_CONTAINER_IMAGE_TAG=e2e-local                  # tag used by CI build step
B3_UPSTREAM_SHARED_SECRET=e2e-test-upstream-secret
B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false     # published port, simpler for CI
B3_LOCAL_MCP_PORT=27640
LOCAL_GATEWAY_MCP_BEARER_TOKEN=e2e-test-bearer-token
B3_OAUTH2_ENFORCE_HOSTNAME_CHECK=false
```

Non-default ports (27630/27640) avoid collisions with a running local Brain3.

#### `Brain3Process`
- Spawns `brain3 --cli --env-file <path> --brain3-home <temp>` as a `tokio::process::Child`
- Polls `http://127.0.0.1:27630/health` until `200 OK`, with a 30s timeout (image is already built locally — no pull latency)
- `Drop` sends SIGTERM and waits for exit

#### `LocalMcpClient`
- Builds a `reqwest::Client` with a default `Authorization: Bearer e2e-test-bearer-token` header
- Calls `StreamableHttpClientTransport::with_client(client, config)` with `uri = "http://127.0.0.1:27640/mcp"` and `auth_header = Some(HeaderValue::from_static("Bearer e2e-test-bearer-token"))`
- Connects via `().serve(transport).await`

---

## Test body: `e2e_smoke_local_docker`

```rust
#[cfg(feature = "e2e")]
#[tokio::test]
async fn e2e_smoke_local_docker() { ... }
```

Run via:
```
cargo test -p brain3 --test e2e_smoke --features e2e -- --nocapture
```

### Steps

1. **Setup**
   - Create `TempTestDir`, write `.env`, create `<temp>/vault/`
   - Spawn `Brain3Process`, wait for `/health` (30s)

2. **Connect**
   - `LocalMcpClient::connect()` → rmcp handshake
   - `client.list_tools()` → assert known tools are present

3. **Create**
   - `vault_create_overwrite_file` path=`e2e-test/note.md` content=`# E2E Test\nHello world.`
   - Assert success

4. **Read**
   - `vault_read` path=`e2e-test/note.md`
   - Assert content contains `Hello world`

5. **Update**
   - `vault_apply_unified_diff` replacing `Hello world` → `Hello updated world`
   - Assert success
   - `vault_read` again, assert new content

6. **Search**
   - `vault_search` query=`updated world`
   - Assert result references `e2e-test/note.md`

7. **Delete**
   - `vault_delete` path=`e2e-test/note.md` confirm=`true`
   - Assert success
   - `vault_read` on same path → assert error / not-found response

8. **Shutdown and verify cleanup**
   - Drop `LocalMcpClient` (graceful close)
   - Drop `Brain3Process` → SIGTERM brain3 → wait for exit
   - Poll `docker ps -a --filter name=brain3-mcp-vault-tools` for up to 15s
   - Assert no containers remain

---

## CI workflow: `e2e.yml`

Triggers: push on `v*` tags only.

```yaml
name: E2E

on:
  push:
    tags:
      - "v*"

jobs:
  e2e-docker-linux:
    name: E2E smoke — Docker / Linux
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build MCP container image from source
        run: |
          docker build \
            -f ./brain3-mcp-vault-tools/Containerfile \
            -t brain3-mcp-vault-tools:e2e-local \
            ./brain3-mcp-vault-tools

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - name: Build brain3 binary
        run: cargo build -p brain3

      - name: Run E2E smoke tests
        run: |
          cargo test -p brain3 --test e2e_smoke --features e2e \
            -- --nocapture
        timeout-minutes: 5
```

No Docker install step needed — `ubuntu-latest` runners ship with Docker.

---

## Documentation changes

### `README.md` — new section "Running E2E tests"

```markdown
## Running E2E tests

E2E tests start a real Brain3 gateway against a locally-built Docker container.
They require Docker and are not run by default.

**Step 1 — build the MCP container image from source:**
```bash
docker build \
  -f ./brain3-mcp-vault-tools/Containerfile \
  -t brain3-mcp-vault-tools:e2e-local \
  ./brain3-mcp-vault-tools
```

**Step 2 — run the tests:**
```bash
cargo test -p brain3 --test e2e_smoke --features e2e -- --nocapture
```
```

### `AGENTS.MD` — add to the "General" section

```markdown
- For risky changes that touch the gateway server, MCP proxy, container startup, or
  vault-tools, run the E2E smoke test to verify end-to-end behaviour:
  `docker build -f ./brain3-mcp-vault-tools/Containerfile -t brain3-mcp-vault-tools:e2e-local ./brain3-mcp-vault-tools && cargo test -p brain3 --test e2e_smoke --features e2e -- --nocapture`
```

---

## Open questions resolved

| Question | Answer |
|---|---|
| Docker pull vs build from source | Build from source; image tag `e2e-local` |
| Fixed ports | Yes — 27630 (gateway), 27640 (local MCP) |
| `#[ignore]` vs feature flag | Feature flag `--features e2e` |
| CI trigger | Release tags (`v*`) only |
| PRs | No E2E on PRs; manual dev run via README |
