"""OAuth 2.0 authorization code flow with PKCE for MCP clients."""

import base64
import hashlib
import hmac
import html
import logging
import secrets
import time
from urllib.parse import urlencode

from starlette.requests import Request
from starlette.responses import HTMLResponse, JSONResponse, RedirectResponse
from starlette.routing import Route

from . import config

logger = logging.getLogger(__name__)

_auth_codes: dict[str, dict] = {}


def _cleanup_codes() -> None:
    now = time.time()
    expired = [code for code, data in _auth_codes.items() if data["expires_at"] < now]
    for code in expired:
        del _auth_codes[code]


def _login_configured() -> bool:
    return bool(config.USERNAME) and bool(config.PASSWORD)


def _check_credentials(username: str, password: str) -> bool:
    if not _login_configured():
        return False
    return hmac.compare_digest(username, config.USERNAME) and hmac.compare_digest(password, config.PASSWORD)


def _authorize_params_from_mapping(data) -> dict[str, str]:
    return {
        "response_type": str(data.get("response_type", "")),
        "client_id": str(data.get("client_id", "")),
        "redirect_uri": str(data.get("redirect_uri", "")),
        "state": str(data.get("state", "")),
        "code_challenge": str(data.get("code_challenge", "")),
        "code_challenge_method": str(data.get("code_challenge_method", "S256")),
    }


def _authorize_request_error(params: dict[str, str]) -> JSONResponse | None:
    if params["response_type"] != "code":
        return JSONResponse({"error": "unsupported_response_type"}, status_code=400)

    if params["client_id"] != config.OAUTH2_GATEWAY_CLIENT_ID:
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    if not params["redirect_uri"]:
        return JSONResponse({"error": "invalid_request", "error_description": "redirect_uri required"}, status_code=400)

    if config.OAUTH2_PKCE_REQUIRED:
        if not params["code_challenge"]:
            return JSONResponse({"error": "invalid_request", "error_description": "code_challenge required"}, status_code=400)
        if params["code_challenge_method"] != "S256":
            return JSONResponse(
                {"error": "invalid_request", "error_description": "code_challenge_method must be S256"},
                status_code=400,
            )

    return None


def _issue_code_redirect(params: dict[str, str]) -> RedirectResponse:
    _cleanup_codes()
    code = secrets.token_urlsafe(32)
    _auth_codes[code] = {
        "client_id": params["client_id"],
        "redirect_uri": params["redirect_uri"],
        "code_challenge": params["code_challenge"],
        "code_challenge_method": params["code_challenge_method"],
        "pkce_required": config.OAUTH2_PKCE_REQUIRED,
        "expires_at": time.time() + 300,
    }

    logger.info("OAuth authorization code issued, redirecting to %s...", params["redirect_uri"][:50])

    redirect_params = {"code": code}
    if params["state"]:
        redirect_params["state"] = params["state"]

    separator = "&" if "?" in params["redirect_uri"] else "?"
    return RedirectResponse(
        url=f'{params["redirect_uri"]}{separator}{urlencode(redirect_params)}',
        status_code=302,
    )


def _login_form(params: dict[str, str], error: str = "", status_code: int = 200) -> HTMLResponse:
    hidden_fields = "\n".join(
        f'<input type="hidden" name="{html.escape(key, quote=True)}" value="{html.escape(value, quote=True)}">'
        for key, value in params.items()
    )
    error_html = (
        f'<p style="color:#b91c1c;font-weight:600;">{html.escape(error)}</p>'
        if error
        else ""
    )
    body = f"""<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Sign in to continue connecting your AI app</title>
  </head>
  <body style="font-family: sans-serif; max-width: 36rem; margin: 3rem auto; line-height: 1.5;">
    <h1>Sign in to continue connecting your AI app</h1>
    <p>ChatGPT, Claude, or another AI app is connecting to your local MCP gateway.</p>
    <p>Use the <code>USERNAME</code> and <code>PASSWORD</code> values from the <code>.env</code> file you configured earlier on this machine.</p>
    {error_html}
    <form method="post" action="/oauth/authorize">
      {hidden_fields}
      <label for="username">Username</label><br>
      <input id="username" name="username" type="text" autocomplete="username" required><br><br>
      <label for="password">Password</label><br>
      <input id="password" name="password" type="password" autocomplete="current-password" required><br><br>
      <button type="submit">Continue</button>
    </form>
  </body>
</html>"""
    return HTMLResponse(body, status_code=status_code)


def _misconfigured_page() -> HTMLResponse:
    body = """<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Login credentials not configured</title>
  </head>
  <body style="font-family: sans-serif; max-width: 36rem; margin: 3rem auto; line-height: 1.5;">
    <h1>Login credentials not configured</h1>
    <p>This gateway requires a login before ChatGPT, Claude, or another AI app can finish connecting.</p>
    <p>Set <code>USERNAME</code> and <code>PASSWORD</code> in the <code>.env</code> file you configured earlier, then restart the gateway.</p>
  </body>
</html>"""
    return HTMLResponse(body, status_code=503)


async def oauth_metadata(request: Request) -> JSONResponse:
    base_url = str(request.base_url).rstrip("/")
    return JSONResponse(
        {
            "issuer": base_url,
            "authorization_endpoint": f"{base_url}/oauth/authorize",
            "token_endpoint": f"{base_url}/oauth/token",
            "grant_types_supported": ["authorization_code"],
            "response_types_supported": ["code"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["client_secret_post"],
        }
    )


async def oauth_authorize(request: Request) -> JSONResponse | RedirectResponse | HTMLResponse:
    source = request.query_params if request.method == "GET" else await request.form()
    params = _authorize_params_from_mapping(source)

    error = _authorize_request_error(params)
    if error is not None:
        return error

    if not _login_configured():
        return _misconfigured_page()

    if request.method == "GET":
        return _login_form(params)

    username = str(source.get("username", ""))
    password = str(source.get("password", ""))
    if not _check_credentials(username, password):
        return _login_form(params, error="Invalid username or password", status_code=401)

    return _issue_code_redirect(params)


async def oauth_token(request: Request) -> JSONResponse:
    try:
        form = await request.form()
    except Exception:
        return JSONResponse({"error": "invalid_request"}, status_code=400)

    grant_type = form.get("grant_type", "")
    client_id = form.get("client_id", "")
    client_secret = form.get("client_secret", "")

    if grant_type == "authorization_code":
        return await _handle_authorization_code(form, client_id, client_secret)

    return JSONResponse({"error": "unsupported_grant_type"}, status_code=400)


async def _handle_authorization_code(form, client_id: str, client_secret: str) -> JSONResponse:
    code = form.get("code", "")
    redirect_uri = form.get("redirect_uri", "")
    code_verifier = form.get("code_verifier", "")

    _cleanup_codes()

    if code not in _auth_codes:
        return JSONResponse({"error": "invalid_grant", "error_description": "Invalid or expired code"}, status_code=400)

    if client_id != config.OAUTH2_GATEWAY_CLIENT_ID:
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    if not config.OAUTH2_GATEWAY_CLIENT_SECRET:
        return JSONResponse({"error": "server_error"}, status_code=500)

    if not hmac.compare_digest(client_secret, config.OAUTH2_GATEWAY_CLIENT_SECRET):
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    code_data = _auth_codes.pop(code)

    if not hmac.compare_digest(client_id, code_data["client_id"]):
        return JSONResponse({"error": "invalid_grant", "error_description": "client_id mismatch"}, status_code=400)

    if redirect_uri and code_data["redirect_uri"] and redirect_uri != code_data["redirect_uri"]:
        return JSONResponse({"error": "invalid_grant", "error_description": "redirect_uri mismatch"}, status_code=400)

    if code_data.get("pkce_required"):
        if not code_data["code_challenge"]:
            return JSONResponse({"error": "invalid_grant", "error_description": "code_challenge required"}, status_code=400)
        if code_data["code_challenge_method"] != "S256":
            return JSONResponse(
                {"error": "invalid_grant", "error_description": "code_challenge_method must be S256"},
                status_code=400,
            )

    if code_data["code_challenge"]:
        if not code_verifier:
            return JSONResponse({"error": "invalid_grant", "error_description": "code_verifier required"}, status_code=400)

        digest = hashlib.sha256(code_verifier.encode("ascii")).digest()
        computed_challenge = base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")
        if not hmac.compare_digest(computed_challenge, code_data["code_challenge"]):
            return JSONResponse({"error": "invalid_grant", "error_description": "PKCE verification failed"}, status_code=400)

    logger.info("OAuth token issued via authorization_code grant")
    return JSONResponse(
        {
            "access_token": config.OAUTH2_GATEWAY_ACCESS_TOKEN,
            "token_type": "bearer",
            "expires_in": 86400,
        }
    )


oauth_routes = [
    Route("/.well-known/oauth-authorization-server", oauth_metadata, methods=["GET"]),
    Route("/oauth/authorize", oauth_authorize, methods=["GET", "POST"]),
    Route("/oauth/token", oauth_token, methods=["POST"]),
]
