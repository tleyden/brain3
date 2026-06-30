"""In-memory index of YAML frontmatter across all vault .md files."""

import logging
import threading
import time
from pathlib import Path

import frontmatter
from watchdog.events import FileSystemEvent, FileSystemEventHandler
from watchdog.observers import Observer

from . import config

logger = logging.getLogger(__name__)

INOTIFY_LIMIT_PATHS = (
    "/proc/sys/fs/inotify/max_user_watches",
    "/proc/sys/fs/inotify/max_user_instances",
)


class FrontmatterIndex:
    """Thread-safe in-memory index of YAML frontmatter for fast queries."""

    def __init__(self, enable_sync_mode: bool = False) -> None:
        self._index: dict[str, dict] = {}
        self._lock = threading.Lock()
        self._observer: Observer | None = None
        self._debounce_timer: threading.Timer | None = None
        self._pending_paths: set[str] = set()
        self._started = False
        self._sync_mode = enable_sync_mode

    def start(self) -> None:
        """Walk all .md files, parse frontmatter, and start watching for changes (unless sync mode)."""
        with self._lock:
            if self._started:
                return
            self._started = True

        t0 = time.monotonic()
        count = 0
        new_index: dict[str, dict] = {}
        observer: Observer | None = None

        try:
            logger.info(
                "Frontmatter index startup scan beginning: vault_path=%s vault_exists=%s "
                "excluded_dirs=%s debounce_seconds=%.2f sync_mode=%s",
                config.VAULT_PATH,
                config.VAULT_PATH.exists(),
                sorted(config.EXCLUDED_DIRS),
                config.FRONTMATTER_INDEX_DEBOUNCE,
                self._sync_mode,
            )
            for md_path in config.VAULT_PATH.rglob("*.md"):
                if self._is_excluded(md_path):
                    logger.info(
                        "Frontmatter startup scan skipped excluded file: path=%s",
                        _display_path(str(md_path)),
                    )
                    continue
                rel = str(md_path.relative_to(config.VAULT_PATH))
                fm = self._parse_frontmatter(md_path)
                if fm is not None:
                    new_index[rel] = fm
                    count += 1
                    logger.info(
                        "Frontmatter startup scan indexed file: path=%s metadata_keys=%s",
                        rel,
                        _metadata_keys(fm),
                    )
                else:
                    logger.info(
                        "Frontmatter startup scan did not index file: path=%s reason=parse_failed",
                        rel,
                    )

            # Only start file watcher if NOT in sync mode
            if not self._sync_mode:
                observer = Observer()
                handler = _VaultEventHandler(self)
                observer.schedule(handler, str(config.VAULT_PATH), recursive=True)
                logger.info(
                    "Frontmatter observer configured: observer_class=%s emitters=%s "
                    "vault_path=%s inotify_limits=%s",
                    _qualified_class_name(observer),
                    _observer_emitter_classes(observer),
                    config.VAULT_PATH,
                    _read_inotify_limits(),
                )
                observer.start()
            else:
                logger.info(
                    "Frontmatter observer disabled: sync_mode=True (use vault_reindex_frontmatter_sync tool)"
                )

            with self._lock:
                self._index = new_index
                self._observer = observer

            elapsed = time.monotonic() - t0
            logger.info(
                "Frontmatter index built: %d files in %.2f seconds sample_keys=%s sync_mode=%s",
                count,
                elapsed,
                _sample_index_keys(new_index),
                self._sync_mode,
            )
        except Exception:
            if observer is not None:
                observer.stop()
                observer.join()
            with self._lock:
                self._started = False
                self._observer = None
            raise

    def stop(self) -> None:
        """Stop the filesystem observer and cancel any pending debounce."""
        with self._lock:
            if not self._started:
                return
            self._started = False
            debounce_timer = self._debounce_timer
            observer = self._observer
            self._debounce_timer = None
            self._observer = None
            self._pending_paths.clear()

        if debounce_timer is not None:
            debounce_timer.cancel()
        if observer is not None:
            observer.stop()
            observer.join()

    def rebuild(self) -> dict:
        """Synchronously rebuild the entire frontmatter index.

        This is primarily for testing when BRAIN3_ENABLE_SYNC_REINDEX_TOOL is enabled.
        Returns stats about the rebuild operation.
        """
        t0 = time.monotonic()
        count = 0
        new_index: dict[str, dict] = {}

        logger.info(
            "Frontmatter index rebuild beginning: vault_path=%s vault_exists=%s",
            config.VAULT_PATH,
            config.VAULT_PATH.exists(),
        )

        for md_path in config.VAULT_PATH.rglob("*.md"):
            if self._is_excluded(md_path):
                continue
            rel = str(md_path.relative_to(config.VAULT_PATH))
            fm = self._parse_frontmatter(md_path)
            if fm is not None:
                new_index[rel] = fm
                count += 1
                logger.info(
                    "Frontmatter rebuild indexed file: path=%s metadata_keys=%s",
                    rel,
                    _metadata_keys(fm),
                )

        with self._lock:
            self._index = new_index

        elapsed = time.monotonic() - t0
        logger.info(
            "Frontmatter index rebuilt: %d files in %.2f seconds sample_keys=%s",
            count,
            elapsed,
            _sample_index_keys(new_index),
        )

        return {
            "file_count": count,
            "elapsed_seconds": elapsed,
            "sample_keys": _sample_index_keys(new_index, max_files=5),
        }

    @property
    def file_count(self) -> int:
        with self._lock:
            return len(self._index)

    def search_by_field(
        self,
        field: str,
        value: str,
        match_type: str,
        path_prefix: str | None = None,
    ) -> list[dict]:
        """Search frontmatter index by field.

        Args:
            field: Frontmatter key to match against.
            value: Value to compare (ignored for match_type "exists").
            match_type: One of "exact", "contains", "exists".
            path_prefix: If set, only return files whose relative path starts with this.

        Returns:
            List of {"path": relative_path, "frontmatter": dict}.
        """
        results: list[dict] = []
        with self._lock:
            stats = {
                "indexed_files": len(self._index),
                "prefix_considered": 0,
                "prefix_skipped": 0,
                "field_present": 0,
                "field_missing": 0,
                "matched": 0,
            }
            samples: list[dict[str, object]] = []
            for rel_path, fm in self._index.items():
                if path_prefix and not rel_path.startswith(path_prefix):
                    stats["prefix_skipped"] += 1
                    continue
                stats["prefix_considered"] += 1
                has_field = field in fm
                if has_field:
                    stats["field_present"] += 1
                else:
                    stats["field_missing"] += 1
                matched = False
                if match_type == "exists":
                    if has_field:
                        results.append({"path": rel_path, "frontmatter": fm})
                        matched = True
                elif match_type == "exact":
                    if has_field and str(fm[field]) == value:
                        results.append({"path": rel_path, "frontmatter": fm})
                        matched = True
                elif match_type == "contains":
                    if has_field and value.lower() in str(fm[field]).lower():
                        results.append({"path": rel_path, "frontmatter": fm})
                        matched = True
                if matched:
                    stats["matched"] += 1
                if len(samples) < 10:
                    samples.append(
                        {
                            "path": rel_path,
                            "has_field": has_field,
                            "field_value": _safe_value_repr(fm.get(field)),
                            "matched": matched,
                            "metadata_keys": _metadata_keys(fm),
                        }
                    )
            logger.info(
                "Frontmatter index search evaluated: field=%r value=%r match_type=%r "
                "path_prefix=%r stats=%s sample=%s",
                field,
                value,
                match_type,
                path_prefix,
                stats,
                samples,
            )
        return results

    def debug_snapshot(
        self, path_prefix: str | None = None, max_files: int = 10
    ) -> dict[str, object]:
        """Return a small diagnostic snapshot of the index state."""
        with self._lock:
            filtered = {
                path: metadata
                for path, metadata in self._index.items()
                if path_prefix is None or path.startswith(path_prefix)
            }
            return {
                "file_count": len(self._index),
                "path_prefix": path_prefix,
                "prefix_file_count": len(filtered),
                "sample_keys": _sample_index_keys(filtered, max_files=max_files),
            }

    # -- Internal helpers --

    def _is_excluded(self, path: Path) -> bool:
        """Check whether any path component is in config.EXCLUDED_DIRS."""
        return bool(
            config.EXCLUDED_DIRS & set(path.relative_to(config.VAULT_PATH).parts)
        )

    def _parse_frontmatter(self, path: Path) -> dict | None:
        """Parse YAML frontmatter from a markdown file. Returns None on failure."""
        try:
            post = frontmatter.load(str(path))
            metadata = dict(post.metadata)
            logger.info(
                "Frontmatter parsed file: path=%s exists=%s metadata_keys=%s",
                _display_path(str(path)),
                path.exists(),
                _metadata_keys(metadata),
            )
            return metadata
        except Exception:
            logger.warning("Failed to parse frontmatter: %s", _display_path(str(path)))
            return None

    def _schedule_debounce(self, abs_path: str) -> None:
        """Add a path to the pending set and reset the debounce timer."""
        with self._lock:
            self._pending_paths.add(abs_path)
            if self._debounce_timer is not None:
                self._debounce_timer.cancel()
            self._debounce_timer = threading.Timer(
                config.FRONTMATTER_INDEX_DEBOUNCE, self._flush_pending
            )
            self._debounce_timer.start()
            logger.info(
                "Frontmatter debounce scheduled: epoch=%.6f debounce_seconds=%.2f "
                "path=%s pending_count=%d pending_paths=%s",
                time.time(),
                config.FRONTMATTER_INDEX_DEBOUNCE,
                _display_path(abs_path),
                len(self._pending_paths),
                _display_paths(self._pending_paths),
            )

    def _flush_pending(self) -> None:
        """Process all pending file changes."""
        with self._lock:
            paths = self._pending_paths.copy()
            self._pending_paths.clear()
            self._debounce_timer = None

        logger.info(
            "Frontmatter debounce flush started: epoch=%.6f pending_count=%d pending_paths=%s",
            time.time(),
            len(paths),
            _display_paths(paths),
        )

        for abs_path_str in paths:
            abs_path = Path(abs_path_str)
            rel = str(abs_path.relative_to(config.VAULT_PATH))
            logger.info(
                "Frontmatter debounce processing path: path=%s exists=%s",
                rel,
                abs_path.exists(),
            )
            if abs_path.exists():
                fm = self._parse_frontmatter(abs_path)
                with self._lock:
                    if fm is not None:
                        self._index[rel] = fm
                        logger.info(
                            "Frontmatter index updated path: path=%s metadata_keys=%s file_count=%d",
                            rel,
                            _metadata_keys(fm),
                            len(self._index),
                        )
                    else:
                        self._index.pop(rel, None)
                        logger.info(
                            "Frontmatter index removed path after parse failure: path=%s file_count=%d",
                            rel,
                            len(self._index),
                        )
            else:
                with self._lock:
                    self._index.pop(rel, None)
                    logger.info(
                        "Frontmatter index removed missing path: path=%s file_count=%d",
                        rel,
                        len(self._index),
                    )

        logger.info(
            "Frontmatter debounce flush finished: epoch=%.6f pending_count=%d file_count=%d",
            time.time(),
            len(paths),
            self.file_count,
        )


class _VaultEventHandler(FileSystemEventHandler):
    """Watchdog handler that feeds .md changes into the frontmatter index."""

    def __init__(self, index: FrontmatterIndex) -> None:
        super().__init__()
        self._index = index

    def dispatch(self, event: FileSystemEvent) -> None:
        logger.info(
            "Frontmatter watchdog event: event_type=%s is_directory=%s src_path=%s dest_path=%s",
            event.event_type,
            event.is_directory,
            _display_path(event.src_path),
            _display_path(getattr(event, "dest_path", None)),
        )
        super().dispatch(event)

    def _handle(self, event: FileSystemEvent) -> None:
        if event.is_directory:
            logger.info(
                "Frontmatter watchdog event ignored: reason=directory event_type=%s path=%s",
                event.event_type,
                _display_path(event.src_path),
            )
            return
        path = Path(event.src_path)
        if path.suffix != ".md":
            logger.info(
                "Frontmatter watchdog event ignored: reason=non_markdown event_type=%s path=%s suffix=%s",
                event.event_type,
                _display_path(event.src_path),
                path.suffix,
            )
            return
        if self._index._is_excluded(path):
            logger.info(
                "Frontmatter watchdog event ignored: reason=excluded event_type=%s path=%s",
                event.event_type,
                _display_path(event.src_path),
            )
            return
        logger.info(
            "Frontmatter watchdog event accepted for debounce: event_type=%s path=%s",
            event.event_type,
            _display_path(event.src_path),
        )
        self._index._schedule_debounce(event.src_path)

    def on_created(self, event: FileSystemEvent) -> None:
        self._handle(event)

    def on_modified(self, event: FileSystemEvent) -> None:
        self._handle(event)

    def on_deleted(self, event: FileSystemEvent) -> None:
        self._handle(event)

    def on_moved(self, event: FileSystemEvent) -> None:
        logger.info(
            "Frontmatter watchdog moved event observed: src_path=%s dest_path=%s "
            "src_is_markdown=%s dest_is_markdown=%s dest_exists=%s diagnostic_only=true "
            "scheduled=false",
            _display_path(event.src_path),
            _display_path(getattr(event, "dest_path", None)),
            Path(event.src_path).suffix == ".md",
            Path(getattr(event, "dest_path", "")).suffix == ".md",
            Path(getattr(event, "dest_path", "")).exists(),
        )


def _read_inotify_limits() -> dict[str, str]:
    limits = {}
    for path in INOTIFY_LIMIT_PATHS:
        try:
            limits[Path(path).name] = Path(path).read_text(encoding="utf-8").strip()
        except OSError as exc:
            limits[Path(path).name] = f"unavailable: {exc}"
    return limits


def _observer_emitter_classes(observer: Observer) -> list[str]:
    emitters = getattr(observer, "emitters", None)
    if emitters is None:
        emitters = getattr(observer, "_emitters", [])
    return sorted(_qualified_class_name(emitter) for emitter in emitters)


def _qualified_class_name(value: object) -> str:
    cls = type(value)
    return f"{cls.__module__}.{cls.__name__}"


def _sample_index_keys(
    index: dict[str, dict], max_files: int = 5, max_keys: int = 8
) -> dict[str, list[str]]:
    return {
        rel_path: sorted(str(key) for key in metadata.keys())[:max_keys]
        for rel_path, metadata in list(sorted(index.items()))[:max_files]
    }


def _metadata_keys(metadata: dict) -> list[str]:
    return sorted(str(key) for key in metadata.keys())


def _safe_value_repr(value: object, max_len: int = 80) -> str:
    if value is None:
        return "<missing>"
    rendered = repr(value)
    if len(rendered) > max_len:
        return f"{rendered[:max_len]}..."
    return rendered


def _display_paths(paths: set[str]) -> list[str]:
    return [_display_path(path) for path in sorted(paths)]


def _display_path(path: str | None) -> str | None:
    if path is None:
        return None
    try:
        return str(Path(path).relative_to(config.VAULT_PATH))
    except ValueError:
        return path
