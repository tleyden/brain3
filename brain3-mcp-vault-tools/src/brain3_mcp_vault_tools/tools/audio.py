"""Audio-related tools for the Brain3 MCP server."""

import logging
import tempfile
import time
from pathlib import Path

logger = logging.getLogger(__name__)


def _safe_path_component(value: str, fallback: str) -> str:
    candidate = Path(value).name.strip()
    return candidate or fallback


def save_audio_file(
    audio_data: bytes, extension: str, suggested_filename: str | None = None
) -> str:
    tmp_dir = Path(tempfile.mkdtemp(prefix="brain3_audio_"))
    logger.info("save_audio_file: temp_dir=%s", tmp_dir)
    timestamp = time.strftime("%Y%m%d_%H%M%S")
    stem = _safe_path_component(suggested_filename or "audio", "audio")
    safe_extension = _safe_path_component(extension.lstrip("."), "")
    if not safe_extension:
        raise ValueError("extension must not be empty")
    dest = tmp_dir / f"{stem}_{timestamp}.{safe_extension}"
    dest.write_bytes(audio_data)
    size_bytes = len(audio_data)
    logger.info("save_audio_file: wrote path=%s size_bytes=%d", dest, size_bytes)
    return (
        f"Saved audio file\n"
        f"  path:       {dest}\n"
        f"  size:       {size_bytes} bytes ({size_bytes / 1024:.1f} KB)\n"
        f"  extension:  {safe_extension}\n"
    )
