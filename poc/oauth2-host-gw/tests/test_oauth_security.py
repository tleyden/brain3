from contextlib import ExitStack
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
LOGIN_USERNAME = "operator"
LOGIN_PASSWORD = "password-123"
REDIRECT_URI = "https://chatgpt.com/connector/oauth/test"
CODE_VERIFIER = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
CODE_CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"


class OAuthSecurityTests(unittest.TestCase):
    def _patch_gateway_config(
        self,
        *,
        client_id=CLIENT_ID,
        client_secret=CLIENT_SECRET,
        username=LOGIN_USERNAME,
        password=LOGIN_PASSWORD,
        pkce_required=True,
    ):
        stack = ExitStack()
        stack.enter_context(patch("oauth2_gateway.config.OAUTH2_GATEWAY_CLIENT_ID", client_id, create=True))
        stack.enter_context(patch("oauth2_gateway.config.OAUTH2_GATEWAY_CLIENT_SECRET", client_secret, create=True))
        stack.enter_context(patch("oauth2_gateway.config.USERNAME", username, create=True))
        stack.enter_context(patch("oauth2_gateway.config.PASSWORD", password, create=True))
        stack.enter_context(patch("oauth2_gateway.config.OAUTH2_PKCE_REQUIRED", pkce_required, create=True))
        return stack

    def _authorize_params(self, **overrides):
        params = {
            "response_type": "code",
            "client_id": CLIENT_ID,
            "redirect_uri": REDIRECT_URI,
            "code_challenge": CODE_CHALLENGE,
            "code_challenge_method": "S256",
        }
        params.update(overrides)
        return params

    def _submit_login(self, client, *, username=LOGIN_USERNAME, password=LOGIN_PASSWORD, **overrides):
        data = self._authorize_params(**overrides)
        data["username"] = username
        data["password"] = password
        return client.post("/oauth/authorize", data=data, follow_redirects=False)

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

        with self._patch_gateway_config():
            with TestClient(app) as client:
                authorize = self._submit_login(client)
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

        with self._patch_gateway_config():
            with TestClient(app) as client:
                authorize = self._submit_login(client)
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

    def test_authorization_code_exchange_rejects_client_id_mismatch_with_bound_code(self):
        app = create_app()

        with self._patch_gateway_config(client_id="client-one"):
                with TestClient(app) as client:
                    authorize = self._submit_login(client, client_id="client-one")
                    code = authorize.headers["location"].split("code=")[1].split("&")[0]

                    with self._patch_gateway_config(client_id="client-two"):
                        token = client.post(
                            "/oauth/token",
                            data={
                                "grant_type": "authorization_code",
                                "client_id": "client-two",
                                "client_secret": CLIENT_SECRET,
                                "redirect_uri": REDIRECT_URI,
                                "code": code,
                                "code_verifier": CODE_VERIFIER,
                            },
                        )

        self.assertEqual(token.status_code, 400)
        self.assertEqual(token.json()["error"], "invalid_grant")
        self.assertEqual(token.json()["error_description"], "client_id mismatch")

    def test_authorize_shows_login_form_with_credential_guidance(self):
        app = create_app()

        with self._patch_gateway_config():
            with TestClient(app) as client:
                response = client.get("/oauth/authorize", params=self._authorize_params(), follow_redirects=False)

        self.assertEqual(response.status_code, 200)
        self.assertIn("Sign in to continue connecting your AI app", response.text)
        self.assertIn("USERNAME", response.text)
        self.assertIn("PASSWORD", response.text)
        self.assertIn(".env", response.text)
        self.assertIn("ChatGPT", response.text)
        self.assertIn("Claude", response.text)

    def test_authorize_returns_503_when_login_credentials_are_not_configured(self):
        app = create_app()

        with self._patch_gateway_config(password=""):
            with TestClient(app) as client:
                response = client.get("/oauth/authorize", params=self._authorize_params(), follow_redirects=False)

        self.assertEqual(response.status_code, 503)
        self.assertIn("PASSWORD", response.text)
        self.assertIn(".env", response.text)

    def test_authorize_rejects_missing_code_challenge_when_pkce_required(self):
        app = create_app()

        with self._patch_gateway_config(pkce_required=True):
            with TestClient(app) as client:
                response = client.get(
                    "/oauth/authorize",
                    params=self._authorize_params(code_challenge=""),
                    follow_redirects=False,
                )

        self.assertEqual(response.status_code, 400)
        self.assertEqual(response.json()["error"], "invalid_request")
        self.assertEqual(response.json()["error_description"], "code_challenge required")

    def test_authorize_rejects_non_s256_code_challenge_method_when_pkce_required(self):
        app = create_app()

        with self._patch_gateway_config(pkce_required=True):
            with TestClient(app) as client:
                response = client.get(
                    "/oauth/authorize",
                    params=self._authorize_params(code_challenge_method="plain"),
                    follow_redirects=False,
                )

        self.assertEqual(response.status_code, 400)
        self.assertEqual(response.json()["error"], "invalid_request")
        self.assertEqual(response.json()["error_description"], "code_challenge_method must be S256")

    def test_authorize_allows_missing_code_challenge_when_pkce_is_not_required(self):
        app = create_app()

        with self._patch_gateway_config(pkce_required=False):
            with TestClient(app) as client:
                response = client.get(
                    "/oauth/authorize",
                    params=self._authorize_params(code_challenge=""),
                    follow_redirects=False,
                )

        self.assertEqual(response.status_code, 200)
        self.assertIn("Sign in to continue connecting your AI app", response.text)

    def test_authorize_rejects_bad_login_credentials(self):
        app = create_app()

        with self._patch_gateway_config():
            with TestClient(app) as client:
                response = self._submit_login(client, password="wrong-password")

        self.assertEqual(response.status_code, 401)
        self.assertIn("Invalid username or password", response.text)

    def test_authorization_code_exchange_requires_code_verifier_when_pkce_is_required(self):
        app = create_app()

        with self._patch_gateway_config(pkce_required=True):
            with TestClient(app) as client:
                authorize = self._submit_login(client)
                code = authorize.headers["location"].split("code=")[1].split("&")[0]

                token = client.post(
                    "/oauth/token",
                    data={
                        "grant_type": "authorization_code",
                        "client_id": CLIENT_ID,
                        "client_secret": CLIENT_SECRET,
                        "redirect_uri": REDIRECT_URI,
                        "code": code,
                    },
                )

        self.assertEqual(token.status_code, 400)
        self.assertEqual(token.json()["error"], "invalid_grant")
        self.assertEqual(token.json()["error_description"], "code_verifier required")
