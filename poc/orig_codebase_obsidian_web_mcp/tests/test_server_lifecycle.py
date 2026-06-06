"""Lifecycle tests for process-scoped frontmatter index startup."""

import uvicorn

import obsidian_vault_mcp.server as server


class _DummyApp:
    def __init__(self) -> None:
        self.routes = []
        self.middlewares = []

    def add_middleware(self, middleware) -> None:
        self.middlewares.append(middleware)


def test_main_starts_and_stops_process_resources_once(vault_dir, monkeypatch):
    """Server boot should start the index once and stop it once on shutdown."""
    calls: list[object] = []
    app = _DummyApp()

    monkeypatch.setattr(server, "VAULT_PATH", vault_dir)
    monkeypatch.setattr(server, "VAULT_MCP_TOKEN", "test-token")
    monkeypatch.setattr(server.mcp, "streamable_http_app", lambda: app)

    def fake_start():
        calls.append("start")

    def fake_stop():
        calls.append("stop")

    def fake_run(app_arg, **kwargs):
        calls.append(("run", app_arg, kwargs["port"]))

    monkeypatch.setattr(server.frontmatter_index, "start", fake_start)
    monkeypatch.setattr(server.frontmatter_index, "stop", fake_stop)
    monkeypatch.setattr(uvicorn, "run", fake_run)

    server.main()

    assert calls == ["start", ("run", app, server.VAULT_MCP_PORT), "stop"]

