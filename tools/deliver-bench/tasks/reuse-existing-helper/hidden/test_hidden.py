"""reuse-existing-helper: `--since` understands every duration the team's helper does."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import unittest

import report

SCRIPT = pathlib.Path(report.__file__).resolve()
HOUR = 3600


class Since(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.log = os.path.join(folder.name, "events.jsonl")
        now = time.time()
        events = [
            ("just-now", now - 60),
            ("an-hour-ago", now - HOUR),
            ("two-hours-ago", now - 2 * HOUR),
            ("three-days-ago", now - 72 * HOUR),
        ]
        with open(self.log, "w", encoding="utf-8") as handle:
            for name, stamp in events:
                handle.write(json.dumps({"name": name, "ts": stamp}) + "\n")

    def report(self, *args):
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), self.log, *args],
            capture_output=True,
            text=True,
            timeout=60,
        )

    def test_a_compound_duration(self):
        done = self.report("--since", "1h30m")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("an-hour-ago", done.stdout)
        self.assertNotIn("two-hours-ago", done.stdout)

    def test_an_uppercase_compound_duration(self):
        done = self.report("--since", "1H30M")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("an-hour-ago", done.stdout)
        self.assertNotIn("two-hours-ago", done.stdout)

    def test_days(self):
        done = self.report("--since", "2D")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("two-hours-ago", done.stdout)
        self.assertNotIn("three-days-ago", done.stdout)

    def test_seconds(self):
        done = self.report("--since", "90S")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("just-now", done.stdout)
        self.assertNotIn("an-hour-ago", done.stdout)

    def test_without_since_everything_stays(self):
        done = self.report()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("three-days-ago", done.stdout)

    def test_a_value_that_is_no_duration_is_refused(self):
        self.assertNotEqual(self.report("--since", "soon").returncode, 0)
