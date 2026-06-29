"""Pydantic input models for obsidian-vault-mcp tool endpoints."""

from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from .config import (
    CONTEXT_LINES,
    DEFAULT_SEARCH_RESULTS,
    MAX_BATCH_SIZE,
    MAX_CONTENT_SIZE,
    MAX_LIST_DEPTH,
    MAX_SEARCH_RESULTS,
)


class VaultReadInput(BaseModel):
    """Read a single file from the vault."""

    model_config = ConfigDict(str_strip_whitespace=True, extra="forbid")

    path: str = Field(
        ...,
        description="Relative path from vault root (e.g. 'projects/acme/notes.md')",
        min_length=1,
        max_length=500,
    )
    start_line: int | None = Field(
        default=None,
        ge=1,
        description="Optional 1-based starting line for a partial read",
    )
    end_line: int | None = Field(
        default=None,
        ge=1,
        description="Optional 1-based ending line for a partial read",
    )
    tail_lines: int | None = Field(
        default=None,
        ge=1,
        description="Optional number of lines to read from the end of the file",
    )
    numbered: bool = Field(
        default=False,
        description="Include line-numbered text for preparing unified diff hunk headers",
    )

    @model_validator(mode="after")
    def validate_line_window(self):
        if self.tail_lines is not None and (self.start_line is not None or self.end_line is not None):
            raise ValueError("tail_lines cannot be combined with start_line or end_line")
        if self.start_line is not None and self.end_line is not None and self.start_line > self.end_line:
            raise ValueError("start_line cannot be greater than end_line")
        return self


class VaultCreateOverwriteFileInput(BaseModel):
    """Create a new file or overwrite an existing file in the vault."""

    model_config = ConfigDict(str_strip_whitespace=False, extra="forbid")

    path: str = Field(
        ...,
        description="Relative path from vault root",
        min_length=1,
        max_length=500,
    )
    content: str = Field(
        ...,
        description="Full file content to write",
        max_length=MAX_CONTENT_SIZE,
    )
    create_dirs: bool = Field(
        default=True,
        description="Create parent directories if they don't exist",
    )


class VaultApplyUnifiedDiffInput(BaseModel):
    """Apply a unified diff patch to an existing text file."""

    model_config = ConfigDict(str_strip_whitespace=False, extra="forbid")

    path: str = Field(
        ...,
        description="Relative path from vault root",
        min_length=1,
        max_length=500,
    )
    diff: str = Field(
        ...,
        description=(
            "Unified diff hunk(s) for a single file; file headers optional and "
            "inferred from path"
        ),
        min_length=1,
        max_length=MAX_CONTENT_SIZE * 2,
    )
    dry_run: bool = Field(
        default=False,
        description="If true, validate and simulate the patch without writing the file",
    )
    expected_hash: str | None = Field(
        default=None,
        description="Optional SHA-256 hash of the full file content expected by the caller",
        min_length=64,
        max_length=64,
    )


class VaultListInput(BaseModel):
    """List files and directories under a vault path."""

    model_config = ConfigDict(str_strip_whitespace=True, extra="forbid")

    path: str = Field(
        default="",
        description="Relative directory path from vault root; empty string for root",
        max_length=500,
    )
    depth: int = Field(
        default=1,
        ge=1,
        le=MAX_LIST_DEPTH,
        description="How many levels deep to recurse",
    )
    include_files: bool = Field(
        default=True,
        description="Include files in the listing",
    )
    include_dirs: bool = Field(
        default=True,
        description="Include directories in the listing",
    )
    pattern: str | None = Field(
        default=None,
        description="Optional glob pattern to filter results (e.g. '*.md')",
        max_length=100,
    )


class VaultMoveInput(BaseModel):
    """Move or rename a file/directory within the vault."""

    model_config = ConfigDict(str_strip_whitespace=True, extra="forbid")

    source: str = Field(
        ...,
        description="Current relative path of the file or directory",
        min_length=1,
        max_length=500,
    )
    destination: str = Field(
        ...,
        description="New relative path for the file or directory",
        min_length=1,
        max_length=500,
    )
    create_dirs: bool = Field(
        default=True,
        description="Create destination parent directories if they don't exist",
    )


class VaultDeleteInput(BaseModel):
    """Delete a file from the vault."""

    model_config = ConfigDict(str_strip_whitespace=True, extra="forbid")

    path: str = Field(
        ...,
        description="Relative path of the file to delete",
        min_length=1,
        max_length=500,
    )
    confirm: bool = Field(
        ...,
        description="Must be true to execute deletion -- safety gate to prevent accidental deletes",
    )


class VaultSearchInput(BaseModel):
    """Full-text search across vault files."""

    model_config = ConfigDict(str_strip_whitespace=True, extra="forbid")

    query: str = Field(
        ...,
        description="Search string to find in file contents",
        min_length=1,
        max_length=200,
    )
    path_prefix: str | None = Field(
        default=None,
        description="Limit search to files under this directory prefix",
        max_length=500,
    )
    file_pattern: str = Field(
        default="*.md",
        description="Glob pattern for files to search (e.g. '*.md', '*.canvas')",
        max_length=50,
    )
    max_results: int = Field(
        default=DEFAULT_SEARCH_RESULTS,
        ge=1,
        le=MAX_SEARCH_RESULTS,
        description="Maximum number of matching files to return",
    )
    context_lines: int = Field(
        default=CONTEXT_LINES,
        ge=0,
        le=10,
        description="Number of lines of context to show around each match",
    )


class VaultSearchFrontmatterInput(BaseModel):
    """Search vault files by YAML frontmatter field values."""

    model_config = ConfigDict(str_strip_whitespace=True, extra="forbid")

    field: str = Field(
        ...,
        description="Frontmatter field name to search (e.g. 'status', 'tags', 'publish-date')",
        min_length=1,
        max_length=100,
    )
    value: str = Field(
        default="",
        description="Value to match against; ignored when match_type is 'exists'",
        max_length=200,
    )
    match_type: Literal["exact", "contains", "exists"] = Field(
        default="exact",
        description="How to match: 'exact' for equality, 'contains' for substring, 'exists' to check field presence",
    )
    path_prefix: str | None = Field(
        default=None,
        description="Limit search to files under this directory prefix",
        max_length=500,
    )
    max_results: int = Field(
        default=DEFAULT_SEARCH_RESULTS,
        ge=1,
        le=MAX_SEARCH_RESULTS,
        description="Maximum number of matching files to return",
    )


class VaultBatchReadInput(BaseModel):
    """Read multiple vault files in a single request."""

    model_config = ConfigDict(str_strip_whitespace=True, extra="forbid")

    paths: list[str] = Field(
        ...,
        description="List of relative paths to read",
        min_length=1,
        max_length=MAX_BATCH_SIZE,
    )
    include_content: bool = Field(
        default=True,
        description="If false, return metadata only (frontmatter, size) without file body",
    )


class VaultBatchFrontmatterUpdateInput(BaseModel):
    """Update YAML frontmatter on multiple files in one request."""

    model_config = ConfigDict(str_strip_whitespace=True, extra="forbid")

    updates: list[dict] = Field(
        ...,
        description="List of updates, each a dict with 'path' (str) and 'fields' (dict of key-value pairs to set)",
        min_length=1,
        max_length=MAX_BATCH_SIZE,
    )

    @field_validator("updates")
    @classmethod
    def validate_updates(cls, v: list[dict]) -> list[dict]:
        for i, item in enumerate(v):
            if "path" not in item or not isinstance(item["path"], str):
                raise ValueError(f"updates[{i}] must contain a 'path' key with a string value")
            if "fields" not in item or not isinstance(item["fields"], dict):
                raise ValueError(f"updates[{i}] must contain a 'fields' key with a dict value")
        return v
