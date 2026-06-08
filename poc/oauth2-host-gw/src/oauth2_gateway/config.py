import os


def _env_bool(name: str, default: bool) -> bool:
    value = os.environ.get(name)
    if value is None:
        return default
    return value.strip().lower() not in {"0", "false", "no", "off"}


def _normalize_hostname(value: str) -> str:
    return value.strip().strip(".").lower()


def _named_tunnel_host() -> str | None:
    tunnel_name = _normalize_hostname(os.environ.get("CF_TUNNEL_NAME", ""))
    domain = _normalize_hostname(os.environ.get("CF_DOMAIN", ""))
    if not tunnel_name or not domain:
        return None
    return f"{tunnel_name}.{domain}".lower()


def _direct_public_origin_hostname() -> str | None:
    hostname = _normalize_hostname(os.environ.get("DIRECT_PUBLIC_ORIGIN_HOSTNAME", ""))
    return hostname or None


def resolve_expected_host() -> str | None:
    named_tunnel_host = _named_tunnel_host()
    direct_public_origin_host = _direct_public_origin_hostname()

    if named_tunnel_host and direct_public_origin_host:
        raise RuntimeError(
            "Both named Cloudflare tunnel hostname settings (CF_TUNNEL_NAME and CF_DOMAIN) "
            "and DIRECT_PUBLIC_ORIGIN_HOSTNAME are set. Choose only one public hostname configuration."
        )

    return named_tunnel_host or direct_public_origin_host


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
