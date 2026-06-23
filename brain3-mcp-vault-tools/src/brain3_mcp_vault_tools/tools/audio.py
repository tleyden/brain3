"""Audio-related tools for the Brain3 MCP server."""

import logging
import mimetypes
import tempfile
import time
import urllib.request
from pathlib import Path
from urllib.parse import urlparse

from ..models import OpenAIFileReferenceInput

logger = logging.getLogger(__name__)


def _safe_path_component(value: str, fallback: str) -> str:
    candidate = Path(value).name.strip()
    return candidate or fallback


def _guess_suffix(audio_file: OpenAIFileReferenceInput) -> str:
    if audio_file.file_name:
        suffix = Path(audio_file.file_name).suffix
        if suffix:
            return suffix

    if audio_file.mime_type:
        guessed = mimetypes.guess_extension(audio_file.mime_type, strict=False)
        if guessed:
            return guessed

    parsed = urlparse(str(audio_file.download_url))
    return Path(parsed.path).suffix


def _build_destination_filename(audio_file: OpenAIFileReferenceInput) -> str:
    timestamp = time.strftime("%Y%m%d_%H%M%S")
    original_name = _safe_path_component(audio_file.file_name or "audio", "audio")
    suffix = _guess_suffix(audio_file)
    stem = Path(original_name).stem if Path(original_name).stem else "audio"
    return f"{stem}_{timestamp}{suffix}"


def save_audio_file(audio_file: OpenAIFileReferenceInput | dict) -> str:
    audio_file = OpenAIFileReferenceInput.model_validate(audio_file)
    tmp_dir = Path(tempfile.mkdtemp(prefix="brain3_audio_"))
    logger.info("save_audio_file: temp_dir=%s", tmp_dir)

    dest = tmp_dir / _build_destination_filename(audio_file)
    parsed_url = urlparse(str(audio_file.download_url))
    logger.info(
        "save_audio_file: downloading file_id=%s mime_type=%r file_name=%r host=%s path=%s",
        audio_file.file_id,
        audio_file.mime_type,
        audio_file.file_name,
        parsed_url.netloc,
        parsed_url.path,
    )

    bytes_written = 0
    content_length: str | None = None
    with urllib.request.urlopen(str(audio_file.download_url)) as response, dest.open(
        "wb"
    ) as handle:
        content_length = response.headers.get("Content-Length")
        while True:
            chunk = response.read(1024 * 1024)
            if not chunk:
                break
            handle.write(chunk)
            bytes_written += len(chunk)

    logger.info(
        "save_audio_file: downloaded file_id=%s bytes_written=%d content_length=%r path=%s",
        audio_file.file_id,
        bytes_written,
        content_length,
        dest,
    )
    return (
        f"Saved audio file\n"
        f"  path:       {dest}\n"
        f"  size:       {bytes_written} bytes ({bytes_written / 1024:.1f} KB)\n"
        f"  file_id:    {audio_file.file_id}\n"
        f"  mime_type:  {audio_file.mime_type or 'unknown'}\n"
        f"  file_name:  {audio_file.file_name or dest.name}\n"
    )
