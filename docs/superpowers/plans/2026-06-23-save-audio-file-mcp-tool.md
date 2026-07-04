Plan: save_audio_file MCP Tool (Experimental)

### Background

Add an experimental `save_audio_file` MCP tool to the FastMCP server (`brain3-mcp-vault-tools`). The LLM passes audio as base64-encoded bytes; FastMCP transparently decodes `bytes`-typed parameters from base64. The tool writes the raw bytes to a temp file and returns stats (path, size) for manual inspection.

---

### Phase 1 — Implement the tool function

**New file:** `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/tools/audio.py`

```python
import logging
import tempfile
import time
from pathlib import Path

logger = logging.getLogger(__name__)

def save_audio_file(audio_data: bytes, extension: str, suggested_filename: str | None = None) -> str:
    tmp_dir = Path(tempfile.mkdtemp(prefix="brain3_audio_"))
    logger.info("save_audio_file: temp_dir=%s", tmp_dir)
    timestamp = time.strftime("%Y%m%d_%H%M%S")
    stem = suggested_filename if suggested_filename else "audio"
    dest = tmp_dir / f"{stem}_{timestamp}.{extension}"
    dest.write_bytes(audio_data)
    size_bytes = len(audio_data)
    logger.info("save_audio_file: wrote path=%s size_bytes=%d", dest, size_bytes)
    return (
        f"Saved audio file\n"
        f"  path:       {dest}\n"
        f"  size:       {size_bytes} bytes ({size_bytes / 1024:.1f} KB)\n"
        f"  extension:  {extension}\n"
    )
```

Parameters:
- `audio_data: bytes` — FastMCP exposes this as a base64 string in the JSON schema and auto-decodes it
- `extension: str` — file extension indicating format (e.g. `"m4a"`, `"wav"`, `"mp3"`); **required**
- `suggested_filename: str | None` — optional stem for the output filename; defaults to `"audio"` if omitted

`cargo test` equivalent: run `uv run pytest` — no new tests added (experimental tool, not core functionality).

---

### Phase 2 — Register in server.py

**File:** `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py`

1. Add import:
   ```python
   from .tools.audio import save_audio_file as _save_audio_file
   ```

2. Register tool after `vault_delete`:
   ```python
   @mcp.tool(
       name="save_audio_file",
       description="Experimental: receive an audio file as base64-encoded bytes, write it to a temp directory, and return the file path and size stats.",
       annotations={
           "readOnlyHint": False,
           "destructiveHint": False,
           "idempotentHint": False,
           "openWorldHint": False,
       },
   )
   def save_audio_file(
       audio_data: bytes,
       extension: str,
       suggested_filename: str | None = None,
   ) -> str:
       return _save_audio_file(audio_data, extension, suggested_filename)
   ```

No new Pydantic input model needed — parameters are simple primitives.

Run `uv run pytest` to confirm nothing regressed.

---

### Notes

- The `bytes` approach is confirmed by FastMCP internals: `bytes`-typed params appear as `"type": "string"` in the JSON schema and FastMCP base64-decodes them before calling the function.
- Temp directory is a fresh `mkdtemp()` per call — no cleanup, since the purpose is manual inspection.
- No model changes, no new tests, no config changes needed.
