import os
from pathlib import Path

VAULT_PATH = Path(os.environ.get("VAULT_PATH", os.path.expanduser("~/Obsidian/MyVault")))
VAULT_MCP_PORT = int(os.environ.get("VAULT_MCP_PORT", "8420"))

_extra = os.environ.get("VAULT_MCP_ALLOWED_HOSTS", "")
VAULT_MCP_EXTRA_ALLOWED_HOSTS: list[str] = [host.strip() for host in _extra.split(",") if host.strip()]

MAX_CONTENT_SIZE = 1_000_000
MAX_BATCH_SIZE = 20
MAX_SEARCH_RESULTS = 50
DEFAULT_SEARCH_RESULTS = 20
MAX_LIST_DEPTH = 5
CONTEXT_LINES = 2

EXCLUDED_DIRS = {".obsidian", ".trash", ".git", ".DS_Store"}
FRONTMATTER_INDEX_DEBOUNCE = 5.0
RATE_LIMIT_READ = 100
RATE_LIMIT_WRITE = 30
