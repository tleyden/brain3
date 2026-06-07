import warnings
import unittest
from unittest.mock import patch

from starlette.exceptions import StarletteDeprecationWarning
from starlette.testclient import TestClient

from oauth2_gateway.server import create_app

warnings.filterwarnings(
    "ignore",
    message=r"Using `httpx` with `starlette\.testclient` is deprecated; install `httpx2` instead\.",
    category=StarletteDeprecationWarning,
)

CLIENT_ID = "oauth2-gateway-client"
CLIENT_SECRET = "hardcoded-secret"
REDIRECT_URI = "https://chatgpt.com/connector/oauth/test"
CODE_VERIFIER = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
CODE_CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"


class OAuthSecurityTests(unittest.TestCase):
    def test_oauth_metadata_only_advertises_preregistered_confidential_client_flow(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.get("/.well-known/oauth-authorization-server")

        self.assertEqual(response.status_code, 200)
        payload = response.json()
        self.assertNotIn("registration_endpoint", payload)
        self.assertEqual(payload["grant_types_supported"], ["authorization_code"])
        self.assertEqual(payload["response_types_supported"], ["code"])
        self.assertEqual(payload["token_endpoint_auth_methods_supported"], ["client_secret_post"])

    def test_oauth_register_route_is_not_exposed(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.post("/oauth/register", json={"client_name": "test"})

        self.assertEqual(response.status_code, 404)

    def test_authorize_rejects_missing_client_id(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.get(
                "/oauth/authorize",
                params={
                    "response_type": "code",
                    "redirect_uri": REDIRECT_URI,
                    "code_challenge": CODE_CHALLENGE,
                    "code_challenge_method": "S256",
                },
                follow_redirects=False,
            )

        self.assertEqual(response.status_code, 401)
        self.assertEqual(response.json()["error"], "invalid_client")

    def test_authorize_rejects_wrong_client_id(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.get(
                "/oauth/authorize",
                params={
                    "response_type": "code",
                    "client_id": "wrong-client",
                    "redirect_uri": REDIRECT_URI,
                    "code_challenge": CODE_CHALLENGE,
                    "code_challenge_method": "S256",
                },
                follow_redirects=False,
            )

        self.assertEqual(response.status_code, 401)
        self.assertEqual(response.json()["error"], "invalid_client")

    def test_authorization_code_exchange_requires_client_secret(self):
        app = create_app()

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_CLIENT_SECRET", CLIENT_SECRET):
            with TestClient(app) as client:
                authorize = client.get(
                    "/oauth/authorize",
                    params={
                        "response_type": "code",
                        "client_id": CLIENT_ID,
                        "redirect_uri": REDIRECT_URI,
                        "code_challenge": CODE_CHALLENGE,
                        "code_challenge_method": "S256",
                    },
                    follow_redirects=False,
                )
                code = authorize.headers["location"].split("code=")[1].split("&")[0]

                missing_secret = client.post(
                    "/oauth/token",
                    data={
                        "grant_type": "authorization_code",
                        "client_id": CLIENT_ID,
                        "redirect_uri": REDIRECT_URI,
                        "code": code,
                        "code_verifier": CODE_VERIFIER,
                    },
                )

        self.assertEqual(missing_secret.status_code, 401)
        self.assertEqual(missing_secret.json()["error"], "invalid_client")

    def test_authorization_code_exchange_succeeds_with_exact_client_id_and_secret(self):
        app = create_app()

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_CLIENT_SECRET", CLIENT_SECRET):
            with TestClient(app) as client:
                authorize = client.get(
                    "/oauth/authorize",
                    params={
                        "response_type": "code",
                        "client_id": CLIENT_ID,
                        "redirect_uri": REDIRECT_URI,
                        "code_challenge": CODE_CHALLENGE,
                        "code_challenge_method": "S256",
                    },
                    follow_redirects=False,
                )
                code = authorize.headers["location"].split("code=")[1].split("&")[0]

                token = client.post(
                    "/oauth/token",
                    data={
                        "grant_type": "authorization_code",
                        "client_id": CLIENT_ID,
                        "client_secret": CLIENT_SECRET,
                        "redirect_uri": REDIRECT_URI,
                        "code": code,
                        "code_verifier": CODE_VERIFIER,
                    },
                )

        self.assertEqual(token.status_code, 200)
        self.assertEqual(token.json()["token_type"], "bearer")
