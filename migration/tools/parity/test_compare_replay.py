"""Exercise the replay workflow's gate without building a server."""
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import compare_replay


class ReplayGate(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        corpus = self.root / "http"
        corpus.mkdir()
        for name, status in (("one", 200), ("two", 403)):
            (corpus / f"{name}.toml").write_text(f"[response]\nstatus = {status}\n")
        self.baseline = "Passed: 1/2\n\n## Failing cases\n\n### one\n"
        self.perfect = "Passed: 2/2\n\nEvery case matches.\n"

    def gate(self, baseline, fresh, status=1):
        old, new = self.root / "old", self.root / "fresh"
        old.write_text(baseline)
        if fresh is not None:
            new.write_text(fresh)
        if fresh is None:
            new.unlink(missing_ok=True)
        with patch.object(sys, "argv", ["gate", "--committed", str(old), "--fresh", str(new),
                                       "--exit-code", str(status), "--summary", str(self.root / "summary")]), \
                contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
            return compare_replay.main()

    def test_reproduced_baselines_and_summary(self):
        self.assertEqual(self.gate(self.baseline, self.baseline), 0)
        self.assertIn("1/2 matched (50.0%); 1 mismatched", (self.root / "summary").read_text())
        self.assertIn("Rails returned 2xx, **0/1** matched", (self.root / "summary").read_text())
        self.assertEqual(self.gate(self.perfect, self.perfect, 0), 0)

    def test_changed_cases_in_either_direction(self):
        self.assertEqual(self.gate(self.baseline, self.perfect, 0), 1)
        self.assertEqual(self.gate(self.perfect, self.baseline), 1)
        self.assertEqual(self.gate(self.baseline, self.baseline.replace("### one", "### two")), 1)

    def test_invalid_report_or_runner_failure(self):
        for invalid in (None, "", "Passed: 2/2\n", self.baseline.replace("1/2", "1/1"),
                        self.baseline.replace("### one", "### three"), self.baseline + "### one\n",
                        self.baseline.replace("### one", ""), "Skipped: 1\n" + self.baseline):
            with self.subTest(invalid=invalid):
                self.assertEqual(self.gate(self.baseline, invalid), 2)
        self.assertEqual(self.gate("", self.baseline), 2)
        self.assertEqual(self.gate(self.baseline, self.baseline, 2), 2)
        self.assertEqual(self.gate(self.perfect, self.perfect, 1), 2)
