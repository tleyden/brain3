"""In-memory storage for auth codes and opaque bearer tokens."""

from __future__ import annotations

from dataclasses import dataclass
import secrets
import time


@dataclass(slots=True)
class AccessToken:
    token: str
    client_id: str
    resource: str
    expires_at: float


class TokenStore:
    def __init__(self) -> None:
        self._tokens: dict[str, AccessToken] = {}

    def issue_access_token(self, *, client_id: str, resource: str) -> AccessToken:
        access_token = AccessToken(
            token=secrets.token_urlsafe(32),
            client_id=client_id,
            resource=resource,
            expires_at=time.time() + 86400,
        )
        self._tokens[access_token.token] = access_token
        return access_token

    def get_access_token(self, token: str) -> AccessToken | None:
        token_data = self._tokens.get(token)
        if token_data is None:
            return None
        if token_data.expires_at < time.time():
            del self._tokens[token]
            return None
        return token_data
