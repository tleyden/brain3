import os


def _env_bool(name: str, default: bool) -> bool:
    value = os.environ.get(name)
    if value is None:
        return default
    return value.strip().lower() not in {"0", "false", "no", "off"}


OAUTH2_GATEWAY_PORT = int(os.environ.get("OAUTH2_GATEWAY_PORT", "8421"))
OAUTH2_GATEWAY_CLIENT_ID = os.environ.get("OAUTH2_GATEWAY_CLIENT_ID", "oauth2-gateway-client")
OAUTH2_GATEWAY_CLIENT_SECRET = os.environ.get("OAUTH2_GATEWAY_CLIENT_SECRET", "")
OAUTH2_GATEWAY_ACCESS_TOKEN = os.environ.get("OAUTH2_GATEWAY_ACCESS_TOKEN", "")
OAUTH2_GATEWAY_MCP_UPSTREAM_URL = os.environ.get("OAUTH2_GATEWAY_MCP_UPSTREAM_URL", "http://127.0.0.1:8420")
OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE = os.environ.get(
    "OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE",
    "/tmp/agentzoo-mcp-upstream-secret",
)
OAUTH2_PKCE_REQUIRED = _env_bool("OAUTH2_PKCE_REQUIRED", True)
USERNAME = os.environ.get("USERNAME", "")
PASSWORD = os.environ.get("PASSWORD", "")
