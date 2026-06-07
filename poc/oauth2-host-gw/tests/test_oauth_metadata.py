import warnings
import unittest

from starlette.exceptions import StarletteDeprecationWarning
from starlette.testclient import TestClient

from oauth2_gateway.server import create_app

warnings.filterwarnings(
    "ignore",
    message=r"Using `httpx` with `starlette\.testclient` is deprecated; install `httpx2` instead\.",
    category=StarletteDeprecationWarning,
)


class OAuthMetadataTests(unittest.TestCase):
    def test_oauth_metadata_advertises_cimd_and_public_client_support(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.get("/.well-known/oauth-authorization-server")

        self.assertEqual(response.status_code, 200)
        payload = response.json()
        self.assertEqual(payload["issuer"], "http://testserver")
        self.assertEqual(payload["authorization_endpoint"], "http://testserver/oauth/authorize")
        self.assertEqual(payload["token_endpoint"], "http://testserver/oauth/token")
        self.assertEqual(payload["registration_endpoint"], "http://testserver/oauth/register")
        self.assertTrue(payload["client_id_metadata_document_supported"])
        self.assertIn("none", payload["token_endpoint_auth_methods_supported"])
        self.assertIn("client_secret_post", payload["token_endpoint_auth_methods_supported"])

    def test_openid_configuration_matches_oauth_metadata(self):
        app = create_app()

        with TestClient(app) as client:
            oauth_response = client.get("/.well-known/oauth-authorization-server")
            openid_response = client.get("/.well-known/openid-configuration")

        self.assertEqual(openid_response.status_code, 200)
        self.assertEqual(openid_response.json(), oauth_response.json())
