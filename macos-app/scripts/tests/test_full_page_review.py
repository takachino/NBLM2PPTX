from __future__ import annotations

import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from full_page_review import digest, render_environment, resolve_evaluation, verify


class ReviewIntegrityTests(unittest.TestCase):
    def test_changed_artifact_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "source.pdf"
            path.write_bytes(b"original")
            pinned = {"path":"source.pdf", "sha256":digest(path), "commit":None}
            path.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError,"SHA-256 mismatch"):
                verify(Path(temp), pinned)

    def test_fontconfig_is_hash_checked_and_only_applied_to_render_children(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            config = Path(temp) / "fonts.conf"
            config.write_text("<fontconfig/>")
            artifact = {"path":"fonts.conf", "sha256":digest(config), "commit":None}
            with patch.dict(os.environ,{"FONTCONFIG_FILE":"existing.conf"}):
                env, record = render_environment(Path(temp),artifact)
                self.assertEqual(env["FONTCONFIG_FILE"],str(config.resolve()))
                self.assertEqual(os.environ["FONTCONFIG_FILE"],"existing.conf")
                self.assertEqual(record["sha256"],artifact["sha256"])
            config.write_text("<fontconfig><dir>different</dir></fontconfig>")
            with self.assertRaisesRegex(ValueError,"SHA-256 mismatch"):
                render_environment(Path(temp),artifact)

    def test_generated_image_paths_follow_the_new_output_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            config = root / "evaluation.json"
            config.write_text('{"checks":[{"metric":{"kind":"image_diff","actual":"@render/backgrounds/after-05.png","expected":"original.png"}},{"metric":{"kind":"json_equal","actual":"diagnostics.json","expected":"literal value"}}]}')
            result = resolve_evaluation(config,root / "new-output")
            self.assertEqual(result["checks"][0]["metric"]["actual"],str(root / "new-output/backgrounds/after-05.png"))
            self.assertEqual(result["checks"][1]["metric"]["expected"],"literal value")


if __name__ == "__main__":
    unittest.main()
