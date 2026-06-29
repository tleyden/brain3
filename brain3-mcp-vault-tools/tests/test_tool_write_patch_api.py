import difflib
import hashlib
import importlib
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

MODULE_PREFIXES = (
    "brain3_mcp_vault_tools.server",
    "brain3_mcp_vault_tools.config",
    "brain3_mcp_vault_tools.vault",
    "brain3_mcp_vault_tools.tools.read",
    "brain3_mcp_vault_tools.tools.write",
    "brain3_mcp_vault_tools.tools.patch",
)


def import_server_module():
    for module_name in tuple(sys.modules):
        if module_name in MODULE_PREFIXES:
            sys.modules.pop(module_name, None)
    return importlib.import_module("brain3_mcp_vault_tools.server")


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


class ToolWritePatchApiTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.vault = Path(self.temp_dir.name)
        self._write_fixture_files()
        self.env_patcher = patch.dict(
            os.environ,
            {
                "B3_VAULT_PATH": str(self.vault),
                "B3_VAULT_MCP_PORT": "2765",
            },
            clear=False,
        )
        self.env_patcher.start()
        self.server = import_server_module()

    def tearDown(self):
        self.env_patcher.stop()
        self.temp_dir.cleanup()

    def _write_fixture_files(self):
        (self.vault / "test-note.md").write_text(
            "---\nstatus: active\ntype: note\n---\n\nLine one.\nLine two.\n",
            encoding="utf-8",
        )

        large_lines = "".join(f"Line {index}\n" for index in range(1, 101))
        (self.vault / "large-note.md").write_text(large_lines, encoding="utf-8")

    def _unified_diff(self, path: str, updated_content: str) -> str:
        original_content = (self.vault / path).read_text(encoding="utf-8")
        original_lines = original_content.splitlines(keepends=True)
        updated_lines = updated_content.splitlines(keepends=True)
        return "".join(
            difflib.unified_diff(
                original_lines,
                updated_lines,
                fromfile=path,
                tofile=path,
                n=3,
            )
        )

    def test_create_overwrite_file_creates_new_file(self):
        result = json.loads(
            self.server.vault_create_overwrite_file("new-note.md", "# Title\n")
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["created"])
        self.assertEqual(
            (self.vault / "new-note.md").read_text(encoding="utf-8"), "# Title\n"
        )

    def test_create_overwrite_file_replaces_existing_file(self):
        result = json.loads(
            self.server.vault_create_overwrite_file(
                "test-note.md", "# Replaced\nBody replaced.\n"
            )
        )

        self.assertNotIn("error", result)
        self.assertFalse(result["created"])
        self.assertEqual(
            (self.vault / "test-note.md").read_text(encoding="utf-8"),
            "# Replaced\nBody replaced.\n",
        )

    def test_vault_read_returns_middle_window_and_full_file_hash(self):
        full_content = (self.vault / "large-note.md").read_text(encoding="utf-8")

        result = json.loads(
            self.server.vault_read("large-note.md", start_line=40, end_line=42)
        )

        self.assertEqual(result["content"], "Line 40\nLine 41\nLine 42\n")
        self.assertEqual(result["returned_start_line"], 40)
        self.assertEqual(result["returned_end_line"], 42)
        self.assertEqual(result["total_lines"], 100)
        self.assertEqual(result["content_hash"], sha256_text(full_content))
        self.assertTrue(result["has_trailing_newline"])
        self.assertNotIn("numbered_lines", result)

    def test_vault_read_numbered_window_returns_line_numbers(self):
        result = json.loads(
            self.server.vault_read(
                "large-note.md", start_line=40, end_line=42, numbered=True
            )
        )

        self.assertEqual(result["content"], "Line 40\nLine 41\nLine 42\n")
        self.assertEqual(result["returned_start_line"], 40)
        self.assertEqual(result["returned_end_line"], 42)
        self.assertEqual(
            result["numbered_lines"],
            [
                {"line": 40, "text": "Line 40"},
                {"line": 41, "text": "Line 41"},
                {"line": 42, "text": "Line 42"},
            ],
        )

    def test_vault_read_returns_tail_window_and_full_file_hash(self):
        full_content = (self.vault / "large-note.md").read_text(encoding="utf-8")

        result = json.loads(self.server.vault_read("large-note.md", tail_lines=2))

        self.assertEqual(result["content"], "Line 99\nLine 100\n")
        self.assertEqual(result["returned_start_line"], 99)
        self.assertEqual(result["returned_end_line"], 100)
        self.assertEqual(result["total_lines"], 100)
        self.assertEqual(result["content_hash"], sha256_text(full_content))

    def test_apply_unified_diff_dry_run_reports_change_without_writing(self):
        updated_content = (
            (self.vault / "large-note.md")
            .read_text(encoding="utf-8")
            .replace(
                "Line 50\n",
                "Updated line 50\n",
            )
        )
        diff_text = self._unified_diff("large-note.md", updated_content)
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")

        result = json.loads(
            self.server.vault_apply_unified_diff(
                "large-note.md", diff_text, dry_run=True
            )
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["dry_run"])
        self.assertTrue(result["would_change"])
        self.assertFalse(result["applied"])
        self.assertEqual(
            (self.vault / "large-note.md").read_text(encoding="utf-8"), original_content
        )

    def test_apply_unified_diff_dry_run_reports_hunk_metadata(self):
        updated_content = (
            (self.vault / "large-note.md")
            .read_text(encoding="utf-8")
            .replace("Line 50\n", "Updated line 50\n")
        )
        diff_text = self._unified_diff("large-note.md", updated_content)

        result = json.loads(
            self.server.vault_apply_unified_diff(
                "large-note.md", diff_text, dry_run=True
            )
        )

        self.assertNotIn("error", result)
        self.assertEqual(
            result["hunks"],
            [
                {
                    "header": "@@ -47,7 +47,7 @@",
                    "old_start": 47,
                    "old_count": 7,
                    "new_count": 7,
                }
            ],
        )

    def test_apply_unified_diff_changes_one_middle_line(self):
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")
        updated_content = original_content.replace("Line 50\n", "Updated line 50\n")
        diff_text = self._unified_diff("large-note.md", updated_content)
        read_result = json.loads(
            self.server.vault_read("large-note.md", start_line=48, end_line=52)
        )

        result = json.loads(
            self.server.vault_apply_unified_diff(
                "large-note.md",
                diff_text,
                expected_hash=read_result["content_hash"],
            )
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])
        self.assertNotIn("hunks", result)
        self.assertEqual(
            (self.vault / "large-note.md").read_text(encoding="utf-8"), updated_content
        )

    def test_apply_unified_diff_accepts_hunk_only_middle_line(self):
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")
        updated_content = original_content.replace("Line 50\n", "Updated line 50\n")
        diff_text = "@@ -50,1 +50,1 @@\n-Line 50\n+Updated line 50\n"

        result = json.loads(
            self.server.vault_apply_unified_diff("large-note.md", diff_text)
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])
        self.assertEqual(
            (self.vault / "large-note.md").read_text(encoding="utf-8"), updated_content
        )

    def test_apply_unified_diff_hunk_only_dry_run_reports_hunk_metadata(self):
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")
        diff_text = "@@ -50,1 +50,1 @@\n-Line 50\n+Updated line 50\n"

        result = json.loads(
            self.server.vault_apply_unified_diff(
                "large-note.md", diff_text, dry_run=True
            )
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["dry_run"])
        self.assertTrue(result["would_change"])
        self.assertFalse(result["applied"])
        self.assertEqual(
            result["hunks"],
            [
                {
                    "header": "@@ -50,1 +50,1 @@",
                    "old_start": 50,
                    "old_count": 1,
                    "new_count": 1,
                }
            ],
        )
        self.assertEqual(
            (self.vault / "large-note.md").read_text(encoding="utf-8"), original_content
        )

    def test_apply_unified_diff_accepts_multi_hunk_hunk_only_patch(self):
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")
        updated_content = original_content.replace(
            "Line 10\n", "Updated line 10\n"
        ).replace("Line 90\n", "Updated line 90\n")
        diff_text = (
            "@@ -10,1 +10,1 @@\n"
            "-Line 10\n"
            "+Updated line 10\n"
            "@@ -90,1 +90,1 @@\n"
            "-Line 90\n"
            "+Updated line 90\n"
        )

        result = json.loads(
            self.server.vault_apply_unified_diff("large-note.md", diff_text)
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])
        self.assertEqual(
            (self.vault / "large-note.md").read_text(encoding="utf-8"), updated_content
        )

    def test_apply_unified_diff_appends_lines_at_end_of_file(self):
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")
        updated_content = original_content + "Append A\nAppend B\n"
        diff_text = self._unified_diff("large-note.md", updated_content)

        result = json.loads(
            self.server.vault_apply_unified_diff("large-note.md", diff_text)
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])
        self.assertEqual(
            (self.vault / "large-note.md").read_text(encoding="utf-8"), updated_content
        )

    def test_apply_unified_diff_rejects_stale_expected_hash(self):
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")
        read_result = json.loads(self.server.vault_read("large-note.md"))

        (self.vault / "large-note.md").write_text(
            original_content + "External change.\n", encoding="utf-8"
        )

        updated_content = original_content.replace("Line 20\n", "Updated line 20\n")
        diff_text = "".join(
            difflib.unified_diff(
                original_content.splitlines(keepends=True),
                updated_content.splitlines(keepends=True),
                fromfile="large-note.md",
                tofile="large-note.md",
                n=3,
            )
        )

        result = json.loads(
            self.server.vault_apply_unified_diff(
                "large-note.md",
                diff_text,
                expected_hash=read_result["content_hash"],
            )
        )

        self.assertEqual(result["error_code"], "hash_mismatch")

    def test_apply_unified_diff_rejects_multi_file_diffs(self):
        diff_text = (
            "--- large-note.md\n"
            "+++ large-note.md\n"
            "@@ -1 +1 @@\n"
            "-a\n"
            "+b\n"
            "--- b.md\n"
            "+++ b.md\n"
            "@@ -1 +1 @@\n"
            "-c\n"
            "+d\n"
        )

        result = json.loads(
            self.server.vault_apply_unified_diff("large-note.md", diff_text)
        )

        self.assertEqual(result["error_code"], "invalid_patch")

    def test_apply_unified_diff_rejects_garbage_prefix_with_clear_message(self):
        diff_text = "not a diff\n@@ -50,1 +50,1 @@\n-Line 50\n+Updated line 50\n"

        result = json.loads(
            self.server.vault_apply_unified_diff("large-note.md", diff_text)
        )

        self.assertEqual(result["error_code"], "invalid_patch")
        self.assertIn(
            "hunk header (@@ ...) or unified diff file headers", result["error"]
        )

    def test_apply_unified_diff_rejects_fenced_patch(self):
        diff_text = (
            "```diff\n"
            "@@ -50,1 +50,1 @@\n"
            "-Line 50\n"
            "+Updated line 50\n"
            "```\n"
        )

        result = json.loads(
            self.server.vault_apply_unified_diff("large-note.md", diff_text)
        )

        self.assertEqual(result["error_code"], "invalid_patch")
        self.assertIn(
            "hunk header (@@ ...) or unified diff file headers", result["error"]
        )

    # --- context_mismatch RCA diagnostic tests ---
    # These tests should FAIL until the fix is applied.
    # Each one constructs a valid patch that should succeed, but currently
    # triggers context_mismatch due to newline handling bugs in _apply_hunks.

    def test_apply_unified_diff_crlf_patch_against_lf_file(self):
        # Scenario A: diff string has \r\n line endings, file has \n.
        # current[1:] gives "hello\r\n"; result_lines entry is "hello\n" → mismatch.
        (self.vault / "crlf-test.md").write_bytes(b"hello\nworld\nfoo\n")
        patch_lf = (
            "--- crlf-test.md\n"
            "+++ crlf-test.md\n"
            "@@ -1,3 +1,4 @@\n"
            " hello\n"
            " world\n"
            "+inserted\n"
            " foo\n"
        )
        patch_crlf = patch_lf.replace("\n", "\r\n")

        result = json.loads(
            self.server.vault_apply_unified_diff("crlf-test.md", patch_crlf)
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])
        self.assertEqual(
            (self.vault / "crlf-test.md").read_bytes(),
            b"hello\nworld\ninserted\nfoo\n",
        )

    def test_apply_unified_diff_patch_missing_trailing_newline(self):
        # Scenario B: the patch string itself does not end with \n.
        # The last context line becomes "foo" (no \n) but the file line is "foo\n".
        (self.vault / "notail-patch.md").write_bytes(b"hello\nworld\nfoo\n")
        patch_no_trailing_nl = (
            "--- notail-patch.md\n"
            "+++ notail-patch.md\n"
            "@@ -1,3 +1,4 @@\n"
            " hello\n"
            " world\n"
            "+inserted\n"
            " foo"  # no trailing \n on the patch string
        )

        result = json.loads(
            self.server.vault_apply_unified_diff("notail-patch.md", patch_no_trailing_nl)
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])
        self.assertEqual(
            (self.vault / "notail-patch.md").read_bytes(),
            b"hello\nworld\ninserted\nfoo\n",
        )

    def test_apply_unified_diff_file_missing_trailing_newline(self):
        # Scenario C: the file does not end with \n.
        # result_lines[-1] = "foo" but patch context text = "foo\n" → mismatch.
        (self.vault / "notail-file.md").write_bytes(b"hello\nworld\nfoo")
        patch = (
            "--- notail-file.md\n"
            "+++ notail-file.md\n"
            "@@ -1,3 +1,4 @@\n"
            " hello\n"
            " world\n"
            "+inserted\n"
            " foo\n"
        )

        result = json.loads(
            self.server.vault_apply_unified_diff("notail-file.md", patch)
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])
        self.assertEqual(
            (self.vault / "notail-file.md").read_bytes(),
            b"hello\nworld\ninserted\nfoo",
        )

    # --- end RCA diagnostic tests ---

    # --- Unicode / emoji context line tests ---
    # The hypothesis: emoji or non-ASCII characters in context lines cause
    # context_mismatch when there is a Unicode normalization mismatch between
    # the file and the patch (NFC vs NFD, or variation-selector presence).
    # Tests 2 and 3 below should FAIL until Unicode normalization is added to
    # the comparison.  Test 1 is the positive control — identical encoding,
    # should already pass.

    def test_apply_unified_diff_emoji_context_identical_encoding(self):
        # Positive control: emoji in context lines, same codepoints in file and
        # patch.  Should succeed with the current code.
        content = "## 7. ⏰ Alarm\nsome content\nmore content\n"
        (self.vault / "emoji-same.md").write_text(content, encoding="utf-8")
        patch = (
            "--- emoji-same.md\n"
            "+++ emoji-same.md\n"
            "@@ -1,3 +1,4 @@\n"
            " ## 7. ⏰ Alarm\n"
            " some content\n"
            "+inserted\n"
            " more content\n"
        )

        result = json.loads(self.server.vault_apply_unified_diff("emoji-same.md", patch))

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])

    def test_apply_unified_diff_nfc_file_nfd_patch_context(self):
        # NFC in file (é = é precomposed), NFD in patch context
        # (é = e + combining accent).  Same visible character, but
        # different codepoint sequences → context_mismatch unless normalization
        # is applied before comparison.
        content = "## Setup\nfoo résumé bar\nsome content\n"
        (self.vault / "nfc-file.md").write_text(content, encoding="utf-8")
        patch = (
            "--- nfc-file.md\n"
            "+++ nfc-file.md\n"
            "@@ -1,3 +1,4 @@\n"
            " ## Setup\n"
            " foo résumé bar\n"   # NFD é
            "+inserted\n"
            " some content\n"
        )

        result = json.loads(self.server.vault_apply_unified_diff("nfc-file.md", patch))

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])

    def test_apply_unified_diff_emoji_variation_selector_mismatch(self):
        # File has bare emoji ⏰ (⏰); patch context has the same emoji
        # followed by variation-selector-16 ️ (⏰️).  Visually identical
        # but different codepoint sequences → context_mismatch unless
        # normalization strips variation selectors before comparison.
        content = "## 7. ⏰ Alarm\nsome content\nmore content\n"
        (self.vault / "emoji-vs.md").write_text(content, encoding="utf-8")
        patch = (
            "--- emoji-vs.md\n"
            "+++ emoji-vs.md\n"
            "@@ -1,3 +1,4 @@\n"
            " ## 7. ⏰️ Alarm\n"   # with variation selector-16
            " some content\n"
            "+inserted\n"
            " more content\n"
        )

        result = json.loads(self.server.vault_apply_unified_diff("emoji-vs.md", patch))

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])

    # --- end Unicode / emoji context line tests ---

    # --- Hunk count mismatch RCA tests ---
    # These tests verify that the parser rejects patches where the @@ header
    # advertises more lines than the hunk body actually contains — the root
    # cause of the "Hunk line counts do not match header counts" failure
    # reported for 2026-Q3.md.  The bug was in the patch generator (client),
    # not in Brain3's application logic; these tests confirm the server already
    # detects and rejects such malformed patches, and that correct patches pass.

    def test_apply_unified_diff_rejects_hunk_count_mismatch_rca_scenario(self):
        # Exact class of failure from the RCA: header claims 3 lines on each
        # side but the body contains only 1 deletion and 1 insertion.
        # @@ -50,3 +50,3 @@ advertises old=3, new=3; body has old=1, new=1.
        diff = (
            "--- large-note.md\n"
            "+++ large-note.md\n"
            "@@ -50,3 +50,3 @@\n"
            "-Line 50\n"
            "+Updated line 50\n"
        )

        result = json.loads(self.server.vault_apply_unified_diff("large-note.md", diff))

        self.assertEqual(result["error_code"], "invalid_patch")
        self.assertIn("Hunk line counts do not match header counts", result["error"])
        self.assertEqual(
            result["details"],
            {
                "header": "@@ -50,3 +50,3 @@",
                "expected_old_count": 3,
                "actual_old_count": 1,
                "expected_new_count": 3,
                "actual_new_count": 1,
                "parsed_hunks": [],
            },
        )

    def test_apply_unified_diff_count_mismatch_reports_prior_parsed_hunks(self):
        diff = (
            "--- large-note.md\n"
            "+++ large-note.md\n"
            "@@ -10,1 +10,1 @@\n"
            "-Line 10\n"
            "+Updated line 10\n"
            "@@ -50,3 +50,3 @@\n"
            "-Line 50\n"
            "+Updated line 50\n"
        )

        result = json.loads(self.server.vault_apply_unified_diff("large-note.md", diff))

        self.assertEqual(result["error_code"], "invalid_patch")
        self.assertEqual(result["details"]["header"], "@@ -50,3 +50,3 @@")
        self.assertEqual(
            result["details"]["parsed_hunks"],
            [
                {
                    "header": "@@ -10,1 +10,1 @@",
                    "old_start": 10,
                    "old_count": 1,
                    "new_count": 1,
                }
            ],
        )

    def test_apply_unified_diff_accepts_minimal_single_line_header(self):
        # Correct minimal patch: @@ -50,1 +50,1 @@ with exactly 1 line each side.
        diff = (
            "--- large-note.md\n"
            "+++ large-note.md\n"
            "@@ -50,1 +50,1 @@\n"
            "-Line 50\n"
            "+Updated line 50\n"
        )

        result = json.loads(self.server.vault_apply_unified_diff("large-note.md", diff))

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])

    def test_apply_unified_diff_accepts_implicit_single_line_header(self):
        # @@ -50 +50 @@ omits the count entirely; the spec treats missing count
        # as 1.  This is the most concise valid form for a single-line swap.
        diff = (
            "--- large-note.md\n"
            "+++ large-note.md\n"
            "@@ -50 +50 @@\n"
            "-Line 50\n"
            "+Updated line 50\n"
        )

        result = json.loads(self.server.vault_apply_unified_diff("large-note.md", diff))

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])

    def test_apply_unified_diff_accepts_single_change_with_matching_context(self):
        # Correct patch with context: @@ -49,3 +49,3 @@ plus 1 context before
        # and 1 context after the changed line (total 3 lines each side).
        diff = (
            "--- large-note.md\n"
            "+++ large-note.md\n"
            "@@ -49,3 +49,3 @@\n"
            " Line 49\n"
            "-Line 50\n"
            "+Updated line 50\n"
            " Line 51\n"
        )

        result = json.loads(self.server.vault_apply_unified_diff("large-note.md", diff))

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])

    def test_apply_unified_diff_rejects_context_lines_omitted_from_body(self):
        # Generator bug variant: header claims 3 context lines but context was
        # dropped from the body, leaving only the changed lines.
        diff = (
            "--- large-note.md\n"
            "+++ large-note.md\n"
            "@@ -49,3 +49,3 @@\n"
            "-Line 50\n"
            "+Updated line 50\n"
            # context lines Line 49 and Line 51 are missing
        )

        result = json.loads(self.server.vault_apply_unified_diff("large-note.md", diff))

        self.assertEqual(result["error_code"], "invalid_patch")
        self.assertIn("Hunk line counts do not match header counts", result["error"])

    # --- end Hunk count mismatch RCA tests ---

    def test_batch_frontmatter_update_preserves_body_content(self):
        result = json.loads(
            self.server.vault_batch_frontmatter_update(
                [{"path": "test-note.md", "fields": {"priority": "high"}}]
            )
        )
        read_result = json.loads(self.server.vault_read("test-note.md"))

        self.assertEqual(result["results"][0]["updated"], True)
        self.assertEqual(read_result["frontmatter"]["priority"], "high")
        self.assertIn("Line one.\nLine two.\n", read_result["content"])


if __name__ == "__main__":
    unittest.main()
