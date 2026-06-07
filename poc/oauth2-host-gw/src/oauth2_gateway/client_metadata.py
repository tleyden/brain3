"""Fetch and validate OAuth Client ID Metadata Documents."""

from __future__ import annotations

from dataclasses import dataclass
from urllib.parse import urlparse

import httpx


@dataclass(slots=True)
class ClientMetadata:
    client_id: str
    client_name: str
    redirect_uris: list[str]
    token_endpoint_auth_method: str


def is_cimd_client_id(client_id: str) -> bool:
    parsed = urlparse(client_id)
    return parsed.scheme == "https" and bool(parsed.netloc) and bool(parsed.path)


async def fetch_client_metadata(client: httpx.AsyncClient, client_id: str) -> ClientMetadata:
    response = await client.get(client_id)
    response.raise_for_status()
    payload = response.json()

    if payload.get("client_id") != client_id:
        raise ValueError("client_id metadata document mismatch")

    redirect_uris = payload.get("redirect_uris")
    if not isinstance(redirect_uris, list) or not redirect_uris:
        raise ValueError("redirect_uris required")

    token_auth_method = payload.get("token_endpoint_auth_method", "none")

    return ClientMetadata(
        client_id=client_id,
        client_name=payload.get("client_name", ""),
        redirect_uris=redirect_uris,
        token_endpoint_auth_method=token_auth_method,
    )
