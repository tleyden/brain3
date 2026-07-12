#!/usr/bin/env python3
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


HOST = "0.0.0.0"
PORT = 8420
SECRET_FILE = Path("/run/secrets/mcp_bearer_token")


def bearer_token() -> str:
    return SECRET_FILE.read_text(encoding="utf-8").strip()


class Handler(BaseHTTPRequestHandler):
    server_version = "brain3-e2e-hello-mcp"

    def do_POST(self) -> None:
        if self.path != "/mcp":
            self.send_json(404, {"error": "not_found"})
            return

        expected = f"Bearer {bearer_token()}"
        supplied = self.headers.get("authorization")
        if supplied is None:
            self.send_json(401, {"error": "missing_bearer"})
            return
        if supplied != expected:
            self.send_json(403, {"error": "wrong_bearer"})
            return

        length = int(self.headers.get("content-length", "0"))
        try:
            request = json.loads(self.rfile.read(length))
        except json.JSONDecodeError:
            self.send_json(400, {"error": "invalid_json"})
            return

        method = request.get("method")
        request_id = request.get("id")
        if method == "initialize":
            result = {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "brain3-e2e-hello-mcp", "version": "0.0.0"},
            }
        elif method == "tools/list":
            result = {
                "tools": [
                    {
                        "name": "hello",
                        "description": "Return a fixed hello-world response.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "additionalProperties": False,
                        },
                    }
                ]
            }
        elif method == "tools/call":
            params = request.get("params") or {}
            if params.get("name") != "hello":
                self.send_json(
                    200,
                    {
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {"code": -32601, "message": "unknown tool"},
                    },
                )
                return
            result = {
                "content": [{"type": "text", "text": "hello world"}],
                "isError": False,
            }
        else:
            self.send_json(
                200,
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32601, "message": "method not found"},
                },
            )
            return

        self.send_json(200, {"jsonrpc": "2.0", "id": request_id, "result": result})

    def log_message(self, format: str, *args: object) -> None:
        return

    def send_json(self, status: int, payload: object) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
