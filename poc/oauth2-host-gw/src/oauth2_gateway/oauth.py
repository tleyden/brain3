"""OAuth 2.0 authorization code flow with PKCE for MCP clients."""

import base64
import hashlib
import hmac
import logging
import secrets
import time
from urllib.parse import urlencode

from starlette.requests import Request
from starlette.responses import JSONResponse, RedirectResponse
from starlette.routing import Route

from . import config

logger = logging.getLogger(__name__)

_auth_codes: dict[str, dict] = {}


def _cleanup_codes() -> None:
    now = time.time()
    expired = [code for code, data in _auth_codes.items() if data["expires_at"] < now]
    for code in expired:
        del _auth_codes[code]


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


async def oauth_authorize(request: Request) -> JSONResponse | RedirectResponse:
    response_type = request.query_params.get("response_type", "")
    client_id = request.query_params.get("client_id", "")
    redirect_uri = request.query_params.get("redirect_uri", "")
    state = request.query_params.get("state", "")
    code_challenge = request.query_params.get("code_challenge", "")
    code_challenge_method = request.query_params.get("code_challenge_method", "S256")

    if response_type != "code":
        return JSONResponse({"error": "unsupported_response_type"}, status_code=400)

    if client_id != config.OAUTH2_GATEWAY_CLIENT_ID:
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    if not redirect_uri:
        return JSONResponse({"error": "invalid_request", "error_description": "redirect_uri required"}, status_code=400)

    _cleanup_codes()
    code = secrets.token_urlsafe(32)
    _auth_codes[code] = {
        "client_id": client_id,
        "redirect_uri": redirect_uri,
        "code_challenge": code_challenge,
        "code_challenge_method": code_challenge_method,
        "expires_at": time.time() + 300,
    }

    logger.info(f"OAuth authorization code issued, redirecting to {redirect_uri[:50]}...")

    params = {"code": code}
    if state:
        params["state"] = state

    separator = "&" if "?" in redirect_uri else "?"
    return RedirectResponse(url=f"{redirect_uri}{separator}{urlencode(params)}", status_code=302)


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
    Route("/oauth/authorize", oauth_authorize, methods=["GET"]),
    Route("/oauth/token", oauth_token, methods=["POST"]),
]
